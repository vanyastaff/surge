//! Allowlisted contracts for durable daemon bootstrap operations.
//!
//! These types contain references and fingerprints, never executable engine
//! configuration, credential values, or environment maps.

use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{ContentHash, RunId, budget::BudgetGuard};

/// Current client-intent and captured-input format.
pub const BOOTSTRAP_VERSION: u32 = 1;

/// Invalid bootstrap input. Messages intentionally exclude supplied values.
#[derive(Debug, thiserror::Error)]
pub enum BootstrapInputError {
    /// A future format must not be guessed to have version-one semantics.
    #[error("unsupported bootstrap payload version")]
    UnsupportedVersion,
    /// An absolute lexical path is required; filesystem canonicalization is capture work.
    #[error("bootstrap paths must be absolute, UTF-8, and contain no parent components")]
    InvalidPath,
    /// Empty intent cannot launch useful work.
    #[error("bootstrap prompt must not be blank")]
    EmptyPrompt,
    /// Zero must not silently mean unlimited.
    #[error("bootstrap budget limits must be finite and positive when present")]
    InvalidBudget,
    /// Invalid isolation identity.
    #[error("bootstrap capture requires distinct run IDs and isolated worktree paths")]
    InvalidIsolation,
    /// Git object identifiers are full, lowercase hex SHA-1 or SHA-256.
    #[error("bootstrap base commit must be a full lowercase hexadecimal Git OID")]
    InvalidOid,
    /// References identify content, not inline configuration.
    #[error("bootstrap reference names must be nonempty and contain no control characters")]
    InvalidReference,
    /// Canonical JSON encoding failed.
    #[error("cannot encode bootstrap fingerprint")]
    Encoding(#[from] serde_json::Error),
}

fn valid_path(path: &Path) -> bool {
    path.is_absolute()
        && path.to_str().is_some()
        && !path.components().any(|c| matches!(c, Component::ParentDir))
}

/// Immutable caller intent, independent of captured HEAD/configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "RawIntent")]
pub struct BootstrapIntent {
    version: u32,
    project_path: PathBuf,
    prompt: String,
    budget: BudgetGuard,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawIntent {
    version: u32,
    project_path: PathBuf,
    prompt: String,
    budget: BudgetGuard,
}

impl TryFrom<RawIntent> for BootstrapIntent {
    type Error = BootstrapInputError;

    fn try_from(raw: RawIntent) -> Result<Self, Self::Error> {
        if raw.version != BOOTSTRAP_VERSION {
            return Err(BootstrapInputError::UnsupportedVersion);
        }
        Self::new(raw.project_path, raw.prompt, raw.budget)
    }
}

impl BootstrapIntent {
    /// Validate intent without reading files or mutable runtime configuration.
    pub fn new(
        project_path: PathBuf,
        prompt: String,
        budget: BudgetGuard,
    ) -> Result<Self, BootstrapInputError> {
        if !valid_path(&project_path) {
            return Err(BootstrapInputError::InvalidPath);
        }
        if prompt.trim().is_empty() {
            return Err(BootstrapInputError::EmptyPrompt);
        }
        if budget
            .limits
            .usd
            .is_some_and(|v| !v.is_finite() || v <= 0.0)
            || budget.limits.tokens == Some(0)
            || budget.limits.warn_threshold_pct > 100
        {
            return Err(BootstrapInputError::InvalidBudget);
        }
        let project_path = project_path.components().collect();
        Ok(Self {
            version: BOOTSTRAP_VERSION,
            project_path,
            prompt,
            budget,
        })
    }

    /// Source path supplied by the client; capture later verifies repository identity.
    pub fn project_path(&self) -> &Path {
        &self.project_path
    }
    /// Exact operator text, including significant whitespace.
    pub fn prompt(&self) -> &str {
        &self.prompt
    }
    /// Operation-wide budget, not a fresh budget for every child.
    pub fn budget(&self) -> BudgetGuard {
        self.budget
    }
    /// Versioned, deterministic serialization of caller intent only.
    pub fn fingerprint(&self) -> Result<ContentHash, BootstrapInputError> {
        Ok(ContentHash::compute(&serde_json::to_vec(self)?))
    }
}

/// Reference to captured content. No inline environment or configuration values.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BootstrapContentRef {
    /// Resolver-owned name/path; never interpreted by persistence.
    pub name: String,
    /// Digest of the content or semantic nonsecret configuration.
    pub digest: ContentHash,
}

/// Raw allowlisted capture fields; construct a validated [`BootstrapCapture`] to store them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BootstrapCaptureFields {
    /// Captured format version, independently checked on deserialization.
    pub version: u32,
    /// Planning run identity, reserved once.
    pub planning_run: RunId,
    /// Implementation run identity, reserved once.
    pub implementation_run: RunId,
    /// Canonical source repository.
    pub repository: PathBuf,
    /// Canonical Git common-directory identity; checked again before execution.
    pub git_common_dir: PathBuf,
    /// Full immutable Git commit OID.
    pub base_commit: String,
    /// Exact isolated planning worktree path.
    pub planning_worktree: PathBuf,
    /// Expected managed planning branch identity.
    pub planning_branch: String,
    /// Exact isolated implementation worktree path.
    pub implementation_worktree: PathBuf,
    /// Expected managed implementation branch identity.
    pub implementation_branch: String,
    /// Pinned bootstrap graph.
    pub graph: BootstrapContentRef,
    /// Captured project context, when present.
    pub project_context: Option<BootstrapContentRef>,
    /// Effective profile/registry/capacity identity actually supplied to the engine.
    pub runtime_identity: ContentHash,
    /// Referenced configuration; values stay outside this journal.
    pub configuration: Vec<BootstrapContentRef>,
    /// `env:NAME` or `secret:NAME` references, never credential values.
    pub credential_refs: Vec<String>,
}

/// Validated frozen environment capture, separate from client intent identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "BootstrapCaptureFields", into = "BootstrapCaptureFields")]
pub struct BootstrapCapture(BootstrapCaptureFields);

impl TryFrom<BootstrapCaptureFields> for BootstrapCapture {
    type Error = BootstrapInputError;
    fn try_from(fields: BootstrapCaptureFields) -> Result<Self, Self::Error> {
        if fields.version != BOOTSTRAP_VERSION {
            return Err(BootstrapInputError::UnsupportedVersion);
        }
        for path in [
            &fields.repository,
            &fields.git_common_dir,
            &fields.planning_worktree,
            &fields.implementation_worktree,
        ] {
            if !valid_path(path) {
                return Err(BootstrapInputError::InvalidPath);
            }
        }
        if fields.planning_run == fields.implementation_run
            || fields.planning_worktree == fields.implementation_worktree
            || fields.planning_branch == fields.implementation_branch
            || fields.repository == fields.planning_worktree
            || fields.repository == fields.implementation_worktree
        {
            return Err(BootstrapInputError::InvalidIsolation);
        }
        if !matches!(fields.base_commit.len(), 40 | 64)
            || !fields
                .base_commit
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        {
            return Err(BootstrapInputError::InvalidOid);
        }
        for reference in &fields.credential_refs {
            let Some((kind, name)) = reference.split_once(':') else {
                return Err(BootstrapInputError::InvalidReference);
            };
            if !matches!(kind, "env" | "secret")
                || name.is_empty()
                || !name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
            {
                return Err(BootstrapInputError::InvalidReference);
            }
        }
        let invalid_name = std::iter::once(fields.graph.name.as_str())
            .chain([
                fields.planning_branch.as_str(),
                fields.implementation_branch.as_str(),
            ])
            .chain(fields.project_context.iter().map(|r| r.name.as_str()))
            .chain(fields.configuration.iter().map(|r| r.name.as_str()))
            .chain(fields.credential_refs.iter().map(String::as_str))
            .any(|name| name.trim().is_empty() || name.chars().any(char::is_control));
        if invalid_name {
            return Err(BootstrapInputError::InvalidReference);
        }
        Ok(Self(fields))
    }
}

impl From<BootstrapCapture> for BootstrapCaptureFields {
    fn from(value: BootstrapCapture) -> Self {
        value.0
    }
}

impl BootstrapCapture {
    /// Immutable validated fields.
    pub fn fields(&self) -> &BootstrapCaptureFields {
        &self.0
    }
    /// Pin checked again before explicit retry; never mixes into client intent identity.
    pub fn fingerprint(&self) -> Result<ContentHash, BootstrapInputError> {
        Ok(ContentHash::compute(&serde_json::to_vec(self)?))
    }
}

/// Executable phases retained when a workflow blocks or cancellation is requested.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BootstrapPhase {
    /// Planning awaits admission.
    QueuedPlanning,
    /// Planning isolation/start is being reconciled.
    PreparingPlanning,
    /// Planning run is active or parked.
    Planning,
    /// Validated child intent awaits admission.
    QueuedImplementation,
    /// Child isolation/start is being reconciled.
    PreparingImplementation,
    /// Implementation run is active or parked.
    Implementing,
}

/// Typed reasons for an explicit operator attention state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BootstrapAttentionReason {
    /// Pinned configuration differs from the actual engine runtime.
    ConfigurationChanged,
    /// A required credential reference cannot be resolved.
    MissingCredential,
    /// Engine startup evidence is incomplete.
    PartialStartup,
    /// Isolation ownership cannot be confirmed.
    WorktreeConflict,
    /// Graph or artifacts could not be validated.
    InvalidMaterialization,
    /// Remaining allowance or cost cannot be established.
    BudgetUnconfirmed,
    /// Durable execution outcome cannot be established.
    StorageUnconfirmed,
}

/// Typed operation state. Attention and cancellation retain the blocked phase.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum BootstrapState {
    /// A runnable or preparing phase.
    Pending { phase: BootstrapPhase },
    /// Explicit retry is needed with the original pinned inputs restored.
    NeedsAttention {
        phase: BootstrapPhase,
        run_id: Option<RunId>,
        reason: BootstrapAttentionReason,
    },
    /// Durable stop intent; settlement has not yet been confirmed.
    Cancelling { phase: BootstrapPhase },
    /// Durable implementation success was observed.
    Completed,
    /// Durable failure was observed.
    Failed,
    /// Cancellation settled and no launch remains possible.
    Cancelled,
}

impl BootstrapState {
    /// Phase to retain when blocked/cancelled; terminal outcomes have none.
    pub fn phase(&self) -> Option<BootstrapPhase> {
        match self {
            Self::Pending { phase }
            | Self::NeedsAttention { phase, .. }
            | Self::Cancelling { phase } => Some(*phase),
            Self::Completed | Self::Failed | Self::Cancelled => None,
        }
    }
}

/// Nonsecret status contract shared by desktop and daemon. No environment payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BootstrapOperationStatus {
    /// Client-assigned stable operation identity.
    pub operation_id: RunId,
    /// Reserved planning identity.
    pub planning_run: RunId,
    /// Reserved child identity.
    pub implementation_run: RunId,
    /// Current typed state.
    pub state: BootstrapState,
    /// Typed confirmed result; absent while execution is unconfirmed.
    pub result: Option<crate::bootstrap_continuation::BootstrapTerminal>,
    /// Monotonic compare-and-swap revision.
    pub revision: u64,
    /// Monotonic durable cancellation intent.
    pub cancel_requested: bool,
    /// Stable FIFO admission ordering.
    pub queue_sequence: u64,
    /// Stored payload format; future versions can be inspected but not executed.
    pub payload_version: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intent_deserialization_cannot_bypass_invariants() {
        let path = std::env::temp_dir();
        let valid =
            BootstrapIntent::new(path, "  exact prompt\n".into(), BudgetGuard::default()).unwrap();
        let json = serde_json::to_value(&valid).unwrap();
        assert_eq!(
            serde_json::from_value::<BootstrapIntent>(json.clone()).unwrap(),
            valid
        );
        assert_eq!(valid.prompt(), "  exact prompt\n");
        for (field, value) in [
            ("prompt", serde_json::json!(" ")),
            ("project_path", serde_json::json!("relative")),
            ("version", serde_json::json!(99)),
            ("env", serde_json::json!({"TOKEN":"secret"})),
        ] {
            let mut invalid = json.clone();
            invalid[field] = value;
            assert!(serde_json::from_value::<BootstrapIntent>(invalid).is_err());
        }
    }

    #[test]
    fn exhausted_budget_is_not_unlimited() {
        let mut guard = BudgetGuard::default();
        guard.limits.tokens = Some(0);
        assert!(BootstrapIntent::new(std::env::temp_dir(), "x".into(), guard).is_err());
        guard.limits.tokens = None;
        guard.limits.usd = Some(f64::NAN);
        assert!(BootstrapIntent::new(std::env::temp_dir(), "x".into(), guard).is_err());
    }

    #[test]
    fn capture_deserialization_validates_identity_and_rejects_inline_configuration() {
        let root = std::env::temp_dir();
        let fields = BootstrapCaptureFields {
            version: 1,
            planning_run: RunId::new(),
            implementation_run: RunId::new(),
            repository: root.clone(),
            git_common_dir: root.clone(),
            base_commit: "a".repeat(40),
            planning_worktree: root.join("plan"),
            planning_branch: "surge/plan".into(),
            implementation_worktree: root.join("child"),
            implementation_branch: "surge/child".into(),
            graph: BootstrapContentRef {
                name: "bootstrap@1".into(),
                digest: ContentHash::compute(b"graph"),
            },
            project_context: None,
            runtime_identity: ContentHash::compute(b"runtime"),
            configuration: vec![],
            credential_refs: vec!["env:API_KEY".into(), "secret:agent_auth".into()],
        };
        let valid: BootstrapCapture = fields.try_into().unwrap();
        let json = serde_json::to_value(&valid).unwrap();
        assert_eq!(
            serde_json::from_value::<BootstrapCapture>(json.clone()).unwrap(),
            valid
        );
        for (name, value) in [
            ("planning_run", json["implementation_run"].clone()),
            ("planning_worktree", json["repository"].clone()),
            ("base_commit", serde_json::json!("main")),
            ("version", serde_json::json!(42)),
            ("credential_refs", serde_json::json!(["literal-secret"])),
            ("env", serde_json::json!({"TOKEN": "DO_NOT_PERSIST"})),
            ("memory_store_path", serde_json::json!("/tmp/memory")),
        ] {
            let mut invalid = json.clone();
            invalid[name] = value;
            assert!(
                serde_json::from_value::<BootstrapCapture>(invalid).is_err(),
                "{name}"
            );
        }
    }
}
