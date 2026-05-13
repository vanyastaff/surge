//! Feature Planner driver for `surge feature describe`.
//!
//! The driver intentionally delegates execution to the normal Agent stage so
//! profile resolution, sandbox selection, hooks, produced artifact declarations,
//! and artifact contract validation stay on the same path as graph execution.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use surge_acp::bridge::facade::BridgeFacade;
use surge_core::agent_config::{AgentConfig, ArtifactSource, Binding, TemplateVar};
use surge_core::keys::{NodeKey, ProfileKey};
use surge_core::node::OutcomeDecl;
use surge_core::profile::keyref::parse_key_ref;
use surge_core::run_state::RunMemory;
use surge_core::{
    McpServerRef, ROADMAP_PATCH_SCHEMA_VERSION, RoadmapPatch, RoadmapPatchValidationIssue, RunId,
};
use surge_mcp::McpRegistry;
use surge_persistence::artifacts::ArtifactStore;
use surge_persistence::runs::RunWriter;
use tokio::sync::{Mutex, oneshot};

use crate::engine::hooks::HookExecutor;
use crate::engine::stage::StageError;
use crate::engine::stage::agent::{AgentStageParams, execute_agent_stage};
use crate::engine::tools::ToolDispatcher;
use crate::profile_loader::ProfileRegistry;

const FEATURE_PLANNER_PROFILE: &str = "feature-planner@1.0";
const FEATURE_PLANNER_NODE: &str = "feature_planner";
const PATCHED_OUTCOME: &str = "patched";
const OUT_OF_SCOPE_OUTCOME: &str = "out_of_scope";
const ROADMAP_PATCH_PATH: &str = "roadmap-patch.toml";

/// Shared map used by injected human-input tool calls.
pub type ToolResolutionMap = Arc<Mutex<HashMap<String, oneshot::Sender<serde_json::Value>>>>;

/// Parameters for one Feature Planner run.
pub struct FeaturePlannerParams<'a> {
    /// Free-form user feature request.
    pub request: String,
    /// Current roadmap text passed into the profile prompt.
    pub roadmap: String,
    /// Bridge facade for ACP session lifecycle.
    pub bridge: &'a Arc<dyn BridgeFacade>,
    /// Run writer for stage events.
    pub writer: &'a RunWriter,
    /// Content-addressed artifact store.
    pub artifact_store: &'a ArtifactStore,
    /// Worktree where `roadmap-patch.toml` is produced.
    pub worktree_path: &'a Path,
    /// Dispatcher for non-injected tools.
    pub tool_dispatcher: &'a Arc<dyn ToolDispatcher>,
    /// Current run memory for binding/tool context.
    pub run_memory: &'a RunMemory,
    /// Current run id.
    pub run_id: RunId,
    /// Human-input tool resolution map.
    pub tool_resolutions: &'a ToolResolutionMap,
    /// Timeout for human-input tool calls.
    pub human_input_timeout: Duration,
    /// Optional MCP registry.
    pub mcp_registry: Option<Arc<McpRegistry>>,
    /// Run-level MCP server refs.
    pub mcp_servers: Vec<McpServerRef>,
    /// Profile registry used to resolve Feature Planner.
    pub profile_registry: Arc<ProfileRegistry>,
    /// Hook executor used by the underlying agent stage.
    pub hook_executor: &'a HookExecutor,
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
    /// Profile reference failed to parse or resolve.
    #[error("feature planner profile error: {0}")]
    Profile(String),
    /// Underlying agent stage failed.
    #[error("feature planner stage failed: {0}")]
    Stage(#[from] StageError),
    /// Patch artifact could not be read from the worktree.
    #[error("read roadmap patch artifact failed: {0}")]
    Io(#[from] std::io::Error),
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
/// Returns [`FeaturePlannerError`] if the profile cannot resolve, the agent
/// stage fails, the produced patch is missing, or the patch does not parse.
#[must_use = "await the driver result and handle failures"]
pub async fn run_feature_planner(
    params: FeaturePlannerParams<'_>,
) -> Result<FeaturePlannerResult, FeaturePlannerError> {
    tracing::debug!(
        target: "feature_driver",
        run_id = %params.run_id,
        "feature_planner_start"
    );

    let resolved_profile = resolve_feature_planner_profile(&params.profile_registry)?;
    let declared_outcomes = declared_outcomes(&resolved_profile.profile.outcomes);
    let node = NodeKey::try_from(FEATURE_PLANNER_NODE)
        .map_err(|error| FeaturePlannerError::Profile(error.to_string()))?;
    let agent_config = feature_planner_agent_config(params.request, params.roadmap)?;

    let outcome = execute_agent_stage(AgentStageParams {
        node: &node,
        agent_config: &agent_config,
        declared_outcomes: &declared_outcomes,
        bridge: params.bridge,
        writer: params.writer,
        artifact_store: params.artifact_store,
        worktree_path: params.worktree_path,
        tool_dispatcher: params.tool_dispatcher,
        run_memory: params.run_memory,
        run_id: params.run_id,
        tool_resolutions: params.tool_resolutions,
        human_input_timeout: params.human_input_timeout,
        mcp_registry: params.mcp_registry,
        mcp_servers: params.mcp_servers,
        profile_registry: Some(params.profile_registry.clone()),
        hook_executor: params.hook_executor,
        pending_elevations: crate::engine::elevation::PendingElevations::new(),
    })
    .await?;

    match outcome.as_ref() {
        PATCHED_OUTCOME => read_validated_patch(params.worktree_path).await,
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

fn resolve_feature_planner_profile(
    registry: &ProfileRegistry,
) -> Result<surge_core::profile::registry::ResolvedProfile, FeaturePlannerError> {
    let key_ref = parse_key_ref(FEATURE_PLANNER_PROFILE)
        .map_err(|error| FeaturePlannerError::Profile(error.to_string()))?;
    registry
        .resolve(&key_ref)
        .map_err(|error| FeaturePlannerError::Profile(error.to_string()))
}

fn declared_outcomes(outcomes: &[surge_core::ProfileOutcome]) -> Vec<OutcomeDecl> {
    outcomes
        .iter()
        .map(|outcome| OutcomeDecl {
            id: outcome.id.clone(),
            description: outcome.description.clone(),
            edge_kind_hint: outcome.edge_kind_hint,
            is_terminal: false,
        })
        .collect()
}

fn feature_planner_agent_config(
    request: String,
    roadmap: String,
) -> Result<AgentConfig, FeaturePlannerError> {
    Ok(AgentConfig {
        profile: ProfileKey::try_from(FEATURE_PLANNER_PROFILE)
            .map_err(|error| FeaturePlannerError::Profile(error.to_string()))?,
        prompt_overrides: None,
        tool_overrides: None,
        sandbox_override: None,
        approvals_override: None,
        bindings: vec![
            Binding {
                source: ArtifactSource::Static { content: request },
                target: TemplateVar("request".into()),
                optional: false,
            },
            Binding {
                source: ArtifactSource::Static { content: roadmap },
                target: TemplateVar("roadmap".into()),
                optional: false,
            },
        ],
        rules_overrides: None,
        limits: Default::default(),
        hooks: Vec::new(),
        custom_fields: Default::default(),
    })
}

async fn read_validated_patch(
    worktree_path: &Path,
) -> Result<FeaturePlannerResult, FeaturePlannerError> {
    let patch_path = worktree_path.join(ROADMAP_PATCH_PATH);
    let content = tokio::fs::read_to_string(&patch_path).await?;
    let patch: RoadmapPatch = toml::from_str(&content)?;
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
