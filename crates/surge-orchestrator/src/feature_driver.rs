//! Feature Planner driver for `surge feature describe`.
//!
//! Runs the bundled `feature-planner@1.0` profile through the standalone
//! agent invocation (`crate::agent_invocation`, #99): registry-resolved
//! provider, the declared roadmap-patch contract checked (and corrected
//! in-session) by the #94 outcome-acceptance rule, cancellation that closes
//! the session, and the planner run's log as the recorder. The driver keeps
//! only the domain: the request/roadmap bindings and the patch's shape
//! validation.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use surge_acp::bridge::facade::BridgeFacade;
use surge_core::keys::NodeKey;
use surge_core::{ROADMAP_PATCH_SCHEMA_VERSION, RoadmapPatch, RoadmapPatchValidationIssue, RunId};
use surge_persistence::runs::RunWriter;
use tokio_util::sync::CancellationToken;

use crate::agent_invocation::{
    DEFAULT_MAX_CONTRACT_REJECTIONS, InvocationFailure, InvocationRecorder, InvocationRequest,
    invoke,
};
use crate::profile_loader::ProfileRegistry;

const FEATURE_PLANNER_PROFILE: &str = "feature-planner@1.0";
const FEATURE_PLANNER_NODE: &str = "feature_planner";
const PATCHED_OUTCOME: &str = "patched";
const OUT_OF_SCOPE_OUTCOME: &str = "out_of_scope";
const ROADMAP_PATCH_PATH: &str = "roadmap-patch.toml";

/// How long the planner may take, from the first send to an accepted patch.
pub const FEATURE_PLANNER_DEADLINE: Duration = Duration::from_secs(30 * 60);

/// Parameters for one Feature Planner run.
pub struct FeaturePlannerParams<'a> {
    /// Free-form user feature request.
    pub request: String,
    /// Current roadmap text passed into the profile prompt.
    pub roadmap: String,
    /// Bridge facade for ACP session lifecycle.
    pub bridge: &'a Arc<dyn BridgeFacade>,
    /// The planner run's writer: the outcome and patch are recorded there.
    pub writer: &'a RunWriter,
    /// Worktree where `roadmap-patch.toml` is produced.
    pub worktree_path: &'a Path,
    /// Current run id (for logs).
    pub run_id: RunId,
    /// Profile registry used to resolve Feature Planner.
    pub profile_registry: Arc<ProfileRegistry>,
    /// The operator's merged agent registry; `None` uses the builtins.
    pub agent_registry: Option<Arc<surge_acp::Registry>>,
    /// Stops the planner and closes its session (the CLI wires Ctrl-C).
    pub cancel: &'a CancellationToken,
    /// From the first send to an accepted patch.
    pub deadline: Duration,
}

/// Result of a Feature Planner run.
#[derive(Debug, Clone, PartialEq)]
pub enum FeaturePlannerResult {
    /// A valid roadmap patch was produced.
    Patched {
        /// Parsed patch.
        patch: Box<RoadmapPatch>,
        /// Path to the produced worktree artifact.
        patch_path: PathBuf,
    },
    /// Request does not belong to this roadmap.
    OutOfScope,
}

/// Errors from the Feature Planner driver.
#[derive(Debug, thiserror::Error)]
pub enum FeaturePlannerError {
    /// The planner invocation failed (provider, session, deadline,
    /// cancellation, or an artifact that stayed invalid).
    #[error("feature planner failed: {0}")]
    Invocation(#[from] InvocationFailure),
    /// Node key construction failed.
    #[error("feature planner node error: {0}")]
    Node(String),
    /// The accepted outcome lacks its patch artifact.
    #[error("feature planner reported {PATCHED_OUTCOME} without {ROADMAP_PATCH_PATH}")]
    MissingPatch,
    /// Patch artifact is not UTF-8.
    #[error("roadmap patch artifact is not UTF-8")]
    NotUtf8,
    /// Patch artifact did not parse as typed TOML.
    #[error("parse roadmap patch artifact failed: {0}")]
    Toml(#[from] toml::de::Error),
    /// Feature Planner reported an unexpected outcome.
    #[error("feature planner returned unexpected outcome: {0}")]
    UnexpectedOutcome(String),
    /// Patch parsed but failed pure shape validation.
    #[error("feature planner returned invalid roadmap patch: {issue_count} issue(s)")]
    InvalidPatch {
        /// Number of validation issues.
        issue_count: usize,
        /// Stable validation issues.
        issues: Vec<RoadmapPatchValidationIssue>,
    },
}

/// Run Feature Planner and return a typed patch result.
///
/// # Errors
/// Returns [`FeaturePlannerError`] if the invocation fails, the patch is
/// missing, or the patch does not parse or validate.
#[must_use = "await the driver result and handle failures"]
pub async fn run_feature_planner(
    params: FeaturePlannerParams<'_>,
) -> Result<FeaturePlannerResult, FeaturePlannerError> {
    tracing::debug!(
        target: "feature_driver",
        run_id = %params.run_id,
        "feature_planner_start"
    );
    let node = NodeKey::try_from(FEATURE_PLANNER_NODE)
        .map_err(|error| FeaturePlannerError::Node(error.to_string()))?;
    let outcome = invoke(
        params.bridge,
        InvocationRequest {
            profile: FEATURE_PLANNER_PROFILE,
            bindings: vec![
                ("request".into(), params.request),
                ("roadmap".into(), params.roadmap),
            ],
            working_dir: params.worktree_path,
            deadline: params.deadline,
            cancel: params.cancel,
            recorder: Some(InvocationRecorder {
                writer: params.writer,
                node: &node,
            }),
            profiles: &params.profile_registry,
            agents: params.agent_registry.as_deref(),
            max_contract_rejections: DEFAULT_MAX_CONTRACT_REJECTIONS,
        },
    )
    .await?;

    match outcome.outcome.as_str() {
        PATCHED_OUTCOME => {
            let artifact = outcome
                .artifact(ROADMAP_PATCH_PATH)
                .ok_or(FeaturePlannerError::MissingPatch)?;
            let content =
                std::str::from_utf8(&artifact.bytes).map_err(|_| FeaturePlannerError::NotUtf8)?;
            validated_patch(content, params.worktree_path.join(ROADMAP_PATCH_PATH))
        },
        OUT_OF_SCOPE_OUTCOME => {
            tracing::warn!(
                target: "feature_driver",
                run_id = %params.run_id,
                "feature_planner_out_of_scope"
            );
            Ok(FeaturePlannerResult::OutOfScope)
        },
        other => Err(FeaturePlannerError::UnexpectedOutcome(other.to_owned())),
    }
}

fn validated_patch(
    content: &str,
    patch_path: PathBuf,
) -> Result<FeaturePlannerResult, FeaturePlannerError> {
    let patch: RoadmapPatch = toml::from_str(content)?;
    let issues = patch.validate_shape();
    if !issues.is_empty() {
        return Err(FeaturePlannerError::InvalidPatch {
            issue_count: issues.len(),
            issues,
        });
    }
    tracing::info!(
        target: "feature_driver",
        patch_id = %patch.id,
        schema_version = patch.schema_version,
        expected_schema_version = ROADMAP_PATCH_SCHEMA_VERSION,
        "feature_planner_patch_loaded"
    );
    Ok(FeaturePlannerResult::Patched {
        patch: Box::new(patch),
        patch_path,
    })
}
