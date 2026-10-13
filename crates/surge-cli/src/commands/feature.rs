//! `surge feature` — roadmap amendment workflow.

use std::collections::HashMap;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use clap::{Args, Subcommand, ValueEnum};
use serde::Serialize;
use surge_core::approvals::{
    ApprovalChannel, ApprovalChannelKind, ApprovalDuration, ApprovalPolicy,
};
use surge_core::roadmap_patch::{
    OperatorConflictChoice, RoadmapPatch, RoadmapPatchApprovalDecision, RoadmapPatchConflict,
    RoadmapPatchId, RoadmapPatchStatus, RoadmapPatchTarget,
};
use surge_core::run_event::{EventPayload, RunConfig, VersionedEventPayload};
use surge_core::sandbox::SandboxMode;
use surge_core::{ContentHash, RunId, SurgeConfig};
use surge_git::GitManager;
use surge_orchestrator::engine::DaemonEngineFacade;
use surge_orchestrator::engine::hooks::HookExecutor;
use surge_orchestrator::engine::tools::ToolDispatcher;
use surge_orchestrator::engine::tools::worktree::WorktreeToolDispatcher;
use surge_orchestrator::feature_driver::{
    FeaturePlannerParams, FeaturePlannerResult, ToolResolutionMap, run_feature_planner,
};
use surge_orchestrator::roadmap_amendment::{
    EngineSource, NotAppliedReason, PatchApplyOutcome, PatchApplyPorts, PatchApplyTarget,
    PatchApproval, RoadmapPatchApprovalLoop, RoadmapPatchApprovalResolution, apply_approved_patch,
    assess_patch,
};
use surge_orchestrator::roadmap_document::{parse_roadmap_document, roadmap_identifiers_prompt};
use surge_orchestrator::roadmap_target::{
    RoadmapTargetCandidate, RoadmapTargetResolver, RoadmapTargetSelector,
};
use surge_persistence::artifacts::ArtifactStore;
use surge_persistence::roadmap_patches::{
    RoadmapPatchIndexFilter, RoadmapPatchIndexRecord, RoadmapPatchIndexUpsert,
};
use surge_persistence::runs::Storage;

const PROJECT_ROADMAP_PATH: &str = ".ai-factory/ROADMAP.md";
const FEATURE_PLANNER_TERMINAL_NODE: &str = "feature_planner";

/// Subcommands under `surge feature`.
#[derive(Subcommand, Debug)]
pub enum FeatureCommands {
    /// Describe a follow-up feature and draft a roadmap patch.
    Describe(FeatureDescribeArgs),
    /// List roadmap patches known to this project.
    List(FeatureListArgs),
    /// Show one roadmap patch record.
    Show(FeatureShowArgs),
    /// Reject a pending roadmap patch.
    Reject(FeatureRejectArgs),
    /// Apply an approved roadmap patch that was not applied yet.
    Apply(FeatureApplyArgs),
}

/// Arguments for `surge feature describe`.
#[derive(Args, Debug)]
pub struct FeatureDescribeArgs {
    /// Free-form feature request.
    #[arg(required = true, num_args = 1..)]
    pub prompt: Vec<String>,
    /// Target a specific run roadmap.
    #[arg(long = "run", value_name = "RUN_ID")]
    pub run_id: Option<String>,
    /// Force the project-level roadmap target.
    #[arg(long)]
    pub project: bool,
    /// Worktree used by Feature Planner for generated artifacts.
    #[arg(long)]
    pub worktree: Option<PathBuf>,
    /// Approval behavior after the patch is drafted.
    #[arg(long, value_enum, default_value_t = FeatureApprovalMode::Prompt)]
    pub approval: FeatureApprovalMode,
    /// Conflict resolution to record when approving a conflicted patch.
    #[arg(long, value_enum)]
    pub conflict_choice: Option<FeatureConflictChoiceArg>,
    /// Emit a single JSON object on stdout; progress stays on stderr.
    #[arg(long)]
    pub json: bool,
}

/// Console approval behavior for drafted patches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum FeatureApprovalMode {
    /// Ask on stdin/stdout.
    Prompt,
    /// Approve immediately and attempt apply.
    Approve,
    /// Reject immediately.
    Reject,
    /// Store the pending patch without a decision.
    Store,
}

/// Conflict resolution choice for `surge feature describe`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum FeatureConflictChoiceArg {
    /// Move new work to the next safe milestone.
    #[value(alias = "defer_to_next_milestone")]
    DeferToNextMilestone,
    /// Stop the active run before a later apply.
    #[value(alias = "abort_current_run")]
    AbortCurrentRun,
    /// Create a separate follow-up run for the new work.
    #[value(alias = "create_follow_up_run")]
    CreateFollowUpRun,
    /// Reject the patch.
    #[value(alias = "reject_patch")]
    RejectPatch,
}

impl From<FeatureConflictChoiceArg> for OperatorConflictChoice {
    fn from(value: FeatureConflictChoiceArg) -> Self {
        match value {
            FeatureConflictChoiceArg::DeferToNextMilestone => Self::DeferToNextMilestone,
            FeatureConflictChoiceArg::AbortCurrentRun => Self::AbortCurrentRun,
            FeatureConflictChoiceArg::CreateFollowUpRun => Self::CreateFollowUpRun,
            FeatureConflictChoiceArg::RejectPatch => Self::RejectPatch,
        }
    }
}

/// Arguments for `surge feature list`.
#[derive(Args, Debug)]
pub struct FeatureListArgs {
    /// Filter by lifecycle status.
    #[arg(long, value_enum)]
    pub status: Option<FeaturePatchStatusArg>,
    /// Include patches from every project instead of only the current project.
    #[arg(long)]
    pub all_projects: bool,
    /// Filter by owning run id.
    #[arg(long = "run", value_name = "RUN_ID")]
    pub run_id: Option<String>,
    /// Maximum rows to print.
    #[arg(long, default_value_t = 50)]
    pub limit: usize,
    /// Emit JSON.
    #[arg(long)]
    pub json: bool,
}

/// Arguments for `surge feature show`.
#[derive(Args, Debug)]
pub struct FeatureShowArgs {
    /// Roadmap patch id.
    pub patch_id: String,
    /// Emit JSON.
    #[arg(long)]
    pub json: bool,
}

/// Arguments for `surge feature reject`.
#[derive(Args, Debug)]
pub struct FeatureRejectArgs {
    /// Roadmap patch id.
    pub patch_id: String,
    /// Optional rejection reason.
    #[arg(long)]
    pub reason: Option<String>,
    /// Conflict resolution to persist with the rejection.
    #[arg(long, value_enum)]
    pub conflict_choice: Option<FeatureConflictChoiceArg>,
    /// Emit JSON.
    #[arg(long)]
    pub json: bool,
}

/// Arguments for `surge feature apply`.
#[derive(Args, Debug)]
pub struct FeatureApplyArgs {
    /// Roadmap patch id.
    pub patch_id: String,
    /// Conflict resolution, when the patch now conflicts.
    #[arg(long, value_enum)]
    pub conflict_choice: Option<FeatureConflictChoiceArg>,
    /// Emit JSON.
    #[arg(long)]
    pub json: bool,
}

/// CLI lifecycle status filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum FeaturePatchStatusArg {
    /// Patch has been drafted.
    Drafted,
    /// Patch is waiting for approval.
    PendingApproval,
    /// Patch has been approved but not fully applied.
    Approved,
    /// Patch has been applied.
    Applied,
    /// Patch has been rejected.
    Rejected,
    /// Patch has been superseded.
    Superseded,
}

impl From<FeaturePatchStatusArg> for RoadmapPatchStatus {
    fn from(value: FeaturePatchStatusArg) -> Self {
        match value {
            FeaturePatchStatusArg::Drafted => Self::Drafted,
            FeaturePatchStatusArg::PendingApproval => Self::PendingApproval,
            FeaturePatchStatusArg::Approved => Self::Approved,
            FeaturePatchStatusArg::Applied => Self::Applied,
            FeaturePatchStatusArg::Rejected => Self::Rejected,
            FeaturePatchStatusArg::Superseded => Self::Superseded,
        }
    }
}

/// Top-level dispatcher for `surge feature`.
pub async fn run(command: FeatureCommands) -> Result<()> {
    match command {
        FeatureCommands::Describe(args) => describe_command(args).await,
        FeatureCommands::List(args) => list_command(args).await,
        FeatureCommands::Show(args) => show_command(args).await,
        FeatureCommands::Reject(args) => reject_command(args).await,
        FeatureCommands::Apply(args) => apply_command(args).await,
    }
}

async fn list_command(args: FeatureListArgs) -> Result<()> {
    let storage = Storage::open(&surge_home_dir()?)
        .await
        .context("open storage")?;
    let project_path = if args.all_projects {
        None
    } else {
        Some(load_project_config_for_current_repo()?.1)
    };
    let run_id = args
        .run_id
        .as_deref()
        .map(parse_run_id)
        .transpose()
        .context("parse --run")?;
    let records = storage
        .roadmap_patch_store()
        .list(&RoadmapPatchIndexFilter {
            status: args.status.map(Into::into),
            project_path,
            run_id,
            limit: Some(args.limit),
        })?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&records)?);
    } else {
        print_patch_table(&records);
    }
    Ok(())
}

async fn show_command(args: FeatureShowArgs) -> Result<()> {
    let storage = Storage::open(&surge_home_dir()?)
        .await
        .context("open storage")?;
    let patch_id = parse_patch_id(&args.patch_id)?;
    let Some(record) = storage.roadmap_patch_store().get(&patch_id)? else {
        return Err(anyhow!("roadmap patch not found: {patch_id}"));
    };
    if args.json {
        println!("{}", serde_json::to_string_pretty(&record)?);
    } else {
        print_patch_detail(&record);
    }
    Ok(())
}

async fn reject_command(args: FeatureRejectArgs) -> Result<()> {
    let storage = Storage::open(&surge_home_dir()?)
        .await
        .context("open storage")?;
    let patch_id = parse_patch_id(&args.patch_id)?;
    let record = surge_orchestrator::roadmap_amendment::reject_patch(
        &storage,
        &patch_id,
        args.reason.clone(),
        args.conflict_choice.map(Into::into),
        ApprovalChannelKind::Desktop,
    )
    .await?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&record)?);
    } else {
        println!(
            "patch_id={} status={} decision={}",
            record.patch_id,
            patch_status_label(record.status),
            record.decision.map_or("none", approval_decision_label)
        );
        if let Some(choice) = record.conflict_choice {
            println!("conflict_choice={}", conflict_choice_label(choice));
        }
    }
    Ok(())
}

/// `surge feature apply`: apply an approved patch again, against the
/// target's current roadmap and state.
async fn apply_command(args: FeatureApplyArgs) -> Result<()> {
    let storage = Storage::open(&surge_home_dir()?)
        .await
        .context("open storage")?;
    let patch_id = parse_patch_id(&args.patch_id)?;
    let Some(record) = storage.roadmap_patch_store().get(&patch_id)? else {
        return Err(anyhow!("roadmap patch not found: {patch_id}"));
    };
    let owner_run_id = record
        .run_id
        .ok_or_else(|| anyhow!("roadmap patch {patch_id} has no owning run event log"))?;
    let patch_path = record
        .patch_path
        .clone()
        .ok_or_else(|| anyhow!("roadmap patch {patch_id} has no stored patch file"))?;
    let patch: RoadmapPatch = toml::from_str(
        &tokio::fs::read_to_string(&patch_path)
            .await
            .with_context(|| format!("read {}", patch_path.display()))?,
    )
    .with_context(|| format!("parse {}", patch_path.display()))?;
    let project_root = record.project_path.clone();
    let config = load_project_config_at(&project_root)?;
    let selector = match &record.target {
        RoadmapPatchTarget::ProjectRoadmap { .. } => RoadmapTargetSelector::ProjectFile,
        RoadmapPatchTarget::RunRoadmap { run_id, .. } => {
            RoadmapTargetSelector::Run { run_id: *run_id }
        },
    };
    let target = RoadmapTargetResolver::new(
        storage.clone(),
        &project_root,
        Path::new(PROJECT_ROADMAP_PATH),
    )
    .resolve(selector)
    .await?;
    let worktree = resolve_feature_worktree(None, &target, &project_root)?;
    let roadmap_text = tokio::fs::read_to_string(&target.roadmap_path)
        .await
        .with_context(|| format!("read roadmap {}", target.roadmap_path.display()))?;
    let artifact_store = ArtifactStore::new(storage.home().join("runs"));
    let writer = storage
        .open_run_writer(owner_run_id)
        .await
        .with_context(|| format!("open owning run writer {owner_run_id}"))?;
    let applied = apply_patch(ApplyRequest {
        storage: &storage,
        artifact_store: &artifact_store,
        writer: &writer,
        owner_run: owner_run_id,
        patch: &patch,
        target: &target,
        roadmap_text,
        project_root: &project_root,
        config: &config,
        worktree: &worktree,
        conflict_choice: args.conflict_choice.map(Into::into),
        json: args.json,
    })
    .await;
    writer
        .close()
        .await
        .context("close owning run writer after apply")?;
    let applied = applied?;
    let record = storage
        .roadmap_patch_store()
        .get(&patch_id)?
        .ok_or_else(|| anyhow!("roadmap patch not found: {patch_id}"))?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&record)?);
    } else {
        println!(
            "patch_id={} status={} result={}",
            record.patch_id,
            patch_status_label(record.status),
            applied.message
        );
        if let Some(run_id) = applied.follow_up_run_id {
            println!("follow_up_run_id={run_id}");
        }
    }
    Ok(())
}

async fn describe_command(args: FeatureDescribeArgs) -> Result<()> {
    let prompt = args.prompt.join(" ");
    let (config, project_root) = load_project_config_for_current_repo()?;
    let storage = Storage::open(&surge_home_dir()?)
        .await
        .context("open storage")?;
    let selector = target_selector(&args)?;
    let resolver = RoadmapTargetResolver::new(
        storage.clone(),
        &project_root,
        Path::new(PROJECT_ROADMAP_PATH),
    );
    let target = resolver.resolve(selector).await?;
    let worktree = resolve_feature_worktree(args.worktree.as_deref(), &target, &project_root)?;
    let roadmap_text = tokio::fs::read_to_string(&target.roadmap_path)
        .await
        .with_context(|| format!("read roadmap {}", target.roadmap_path.display()))?;
    let roadmap_prompt = roadmap_prompt_text(&target.roadmap_path, &roadmap_text);

    progress(args.json, format_args!("target={}", target_label(&target)));
    progress(args.json, format_args!("worktree={}", worktree.display()));

    let planner_run_id = RunId::new();
    let writer = storage
        .create_run(planner_run_id, &worktree, Some("feature-planner".into()))
        .await
        .context("create feature planner run")?;
    append_feature_run_started(&writer, &worktree).await?;

    let command_result = run_describe_flow(DescribeFlow {
        args: &args,
        prompt,
        config,
        project_root,
        storage: storage.clone(),
        target,
        worktree,
        roadmap_text,
        roadmap_prompt,
        planner_run_id,
        writer: &writer,
    })
    .await;

    append_feature_run_terminal(&writer, &command_result).await?;
    writer
        .close()
        .await
        .context("close feature planner writer")?;
    command_result
}

struct DescribeFlow<'a> {
    args: &'a FeatureDescribeArgs,
    prompt: String,
    config: SurgeConfig,
    project_root: PathBuf,
    storage: Arc<Storage>,
    target: RoadmapTargetCandidate,
    worktree: PathBuf,
    roadmap_text: String,
    roadmap_prompt: String,
    planner_run_id: RunId,
    writer: &'a surge_persistence::runs::RunWriter,
}

async fn run_describe_flow(flow: DescribeFlow<'_>) -> Result<()> {
    let artifact_store = ArtifactStore::new(flow.storage.home().join("runs"));
    let bridge: Arc<dyn surge_acp::bridge::facade::BridgeFacade> = Arc::new(
        surge_acp::bridge::AcpBridge::with_defaults().context("AcpBridge::with_defaults")?,
    );
    let tool_dispatcher: Arc<dyn ToolDispatcher> =
        Arc::new(WorktreeToolDispatcher::new(flow.worktree.clone()));
    let profile_registry = Arc::new(
        surge_orchestrator::profile_loader::ProfileRegistry::load()
            .context("load profile registry")?,
    );
    let hook_executor = HookExecutor::new();
    let tool_resolutions: ToolResolutionMap = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
    let memory = surge_core::run_state::RunMemory::default();

    progress(
        flow.args.json,
        format_args!("planner_run_id={}", flow.planner_run_id),
    );
    let result = run_feature_planner(FeaturePlannerParams {
        request: flow.prompt.clone(),
        roadmap: flow.roadmap_prompt.clone(),
        bridge: &bridge,
        writer: flow.writer,
        artifact_store: &artifact_store,
        worktree_path: &flow.worktree,
        tool_dispatcher: &tool_dispatcher,
        run_memory: &memory,
        run_id: flow.planner_run_id,
        tool_resolutions: &tool_resolutions,
        human_input_timeout: Duration::from_secs(300),
        mcp_registry: None,
        mcp_servers: Vec::new(),
        profile_registry,
        hook_executor: &hook_executor,
    })
    .await?;

    match result {
        FeaturePlannerResult::OutOfScope => emit_output(
            flow.args.json,
            FeatureDescribeOutput::out_of_scope(flow.planner_run_id, &flow.target),
        ),
        FeaturePlannerResult::Patched { patch, patch_path } => {
            let mut patch = *patch;
            normalize_patch_target(&mut patch, &flow.target);
            let patch = assess_for_approval(
                patch,
                &flow.target.roadmap_path,
                &flow.roadmap_text,
                flow.args.json,
            );
            let patch_toml = toml::to_string_pretty(&patch).context("serialize roadmap patch")?;
            tokio::fs::write(&patch_path, patch_toml.as_bytes())
                .await
                .with_context(|| format!("write normalized patch {}", patch_path.display()))?;
            let patch_hash = patch.content_hash().context("hash roadmap patch")?;
            let draft_artifact = RoadmapPatchApprovalLoop::store_draft(
                &artifact_store,
                flow.writer,
                flow.planner_run_id,
                &patch,
                patch_toml.as_bytes(),
            )
            .await?;
            let mut record = upsert_patch_index(
                &flow.storage,
                &flow.target,
                &patch,
                flow.planner_run_id,
                PatchIndexUpdate {
                    content_hash: patch_hash,
                    status: RoadmapPatchStatus::Drafted,
                    patch_artifact: Some(draft_artifact.hash),
                    patch_path: Some(draft_artifact.path.clone()),
                    summary_hash: None,
                    decision: None,
                    decision_comment: None,
                    conflict_choice: None,
                },
            )?;

            let decision = decide_patch(
                flow.writer,
                &patch,
                flow.args.approval,
                flow.args.conflict_choice.map(Into::into),
            )
            .await?;
            let mut follow_up_run_id = None;
            let mut selected_conflict_choice = decision.conflict_choice();
            let status = match decision {
                PatchDecision::Store { summary_hash } => {
                    record = upsert_patch_index(
                        &flow.storage,
                        &flow.target,
                        &patch,
                        flow.planner_run_id,
                        PatchIndexUpdate {
                            content_hash: patch_hash,
                            status: RoadmapPatchStatus::PendingApproval,
                            patch_artifact: record.patch_artifact,
                            patch_path: record.patch_path.clone(),
                            summary_hash: Some(summary_hash),
                            decision: None,
                            decision_comment: None,
                            conflict_choice: None,
                        },
                    )?;
                    "pending_approval".to_owned()
                },
                PatchDecision::Reject {
                    comment,
                    conflict_choice,
                } => {
                    selected_conflict_choice = conflict_choice;
                    record = upsert_patch_index(
                        &flow.storage,
                        &flow.target,
                        &patch,
                        flow.planner_run_id,
                        PatchIndexUpdate {
                            content_hash: patch_hash,
                            status: RoadmapPatchStatus::Rejected,
                            patch_artifact: record.patch_artifact,
                            patch_path: record.patch_path.clone(),
                            summary_hash: None,
                            decision: Some(RoadmapPatchApprovalDecision::Reject),
                            decision_comment: comment,
                            conflict_choice,
                        },
                    )?;
                    "rejected".to_owned()
                },
                PatchDecision::Approve { conflict_choice } => {
                    let apply = apply_patch(ApplyRequest {
                        storage: &flow.storage,
                        artifact_store: &artifact_store,
                        writer: flow.writer,
                        owner_run: flow.planner_run_id,
                        patch: &patch,
                        target: &flow.target,
                        roadmap_text: flow.roadmap_text.clone(),
                        project_root: &flow.project_root,
                        config: &flow.config,
                        worktree: &flow.worktree,
                        conflict_choice,
                        json: flow.args.json,
                    })
                    .await?;
                    follow_up_run_id = apply.follow_up_run_id;
                    record = flow
                        .storage
                        .roadmap_patch_store()
                        .get(&patch.id)?
                        .ok_or_else(|| anyhow!("roadmap patch not found: {}", patch.id))?;
                    selected_conflict_choice = record.conflict_choice;
                    apply.message
                },
            };

            emit_output(
                flow.args.json,
                FeatureDescribeOutput {
                    status,
                    planner_run_id: flow.planner_run_id.to_string(),
                    patch_id: Some(record.patch_id.to_string()),
                    target: target_label(&flow.target),
                    patch_artifact: record.patch_artifact.map(|hash| hash.to_string()),
                    patch_path: record
                        .patch_path
                        .as_ref()
                        .map(|path| path.display().to_string()),
                    follow_up_run_id: follow_up_run_id.map(|run_id| run_id.to_string()),
                    conflict_choice: selected_conflict_choice
                        .map(conflict_choice_label)
                        .map(str::to_owned),
                    message: output_message(record.status),
                },
            )
        },
    }
}

async fn daemon_engine_facade() -> Result<DaemonEngineFacade> {
    crate::commands::engine::ensure_daemon_running().await?;
    let socket_path = surge_daemon::pidfile::socket_path().context("resolve daemon socket path")?;
    DaemonEngineFacade::connect(socket_path)
        .await
        .context("connect to daemon")
}

/// The engine, connected only when a route needs it.
struct DaemonEngine;

#[async_trait::async_trait]
impl EngineSource for DaemonEngine {
    async fn engine(
        &self,
    ) -> Result<
        Arc<dyn surge_orchestrator::engine::EngineFacade>,
        surge_orchestrator::engine::EngineError,
    > {
        let facade = daemon_engine_facade().await.map_err(|error| {
            surge_orchestrator::engine::EngineError::Internal(format!("{error:#}"))
        })?;
        Ok(Arc::new(facade))
    }
}

struct ApplyRequest<'a> {
    storage: &'a Arc<Storage>,
    artifact_store: &'a ArtifactStore,
    writer: &'a surge_persistence::runs::RunWriter,
    owner_run: RunId,
    patch: &'a RoadmapPatch,
    target: &'a RoadmapTargetCandidate,
    roadmap_text: String,
    project_root: &'a Path,
    config: &'a SurgeConfig,
    worktree: &'a Path,
    conflict_choice: Option<OperatorConflictChoice>,
    json: bool,
}

struct Applied {
    message: String,
    follow_up_run_id: Option<RunId>,
}

/// Apply an approved patch through the orchestrator and describe the result.
async fn apply_patch(request: ApplyRequest<'_>) -> Result<Applied> {
    let project_context = surge_orchestrator::project_context::load_project_context_seed(
        request.project_root,
        request.config,
    );
    let outcome = apply_approved_patch(
        PatchApplyPorts {
            artifact_store: request.artifact_store,
            storage: request.storage,
            writer: request.writer,
            owner_run: request.owner_run,
            engine: &DaemonEngine,
        },
        request.patch,
        PatchApproval {
            conflict_choice: request.conflict_choice,
            comment: None,
            channel: local_approval_channel(),
            channel_used: ApprovalChannelKind::Desktop,
        },
        &PatchApplyTarget {
            target: request.target.target.clone(),
            amendment_point: request.target.amendment_point,
            roadmap_path: request.target.roadmap_path.clone(),
            roadmap_text: request.roadmap_text,
            active_run: request.target.run_id,
            project_path: request.target.project_path.clone(),
            worktree: request.worktree.to_path_buf(),
            project_context,
        },
    )
    .await?;
    let (message, follow_up_run_id) = match &outcome {
        PatchApplyOutcome::AppliedToProjectRoadmap { .. } => ("applied_project_roadmap", None),
        PatchApplyOutcome::AppliedToActiveRun(active) => {
            progress(
                request.json,
                format_args!(
                    "active_run_amended={} graph_hash={}",
                    active.run_id, active.graph_hash
                ),
            );
            ("applied_active_run", None)
        },
        PatchApplyOutcome::FollowUpStarted {
            run_id,
            from_conflict_choice,
        } => {
            progress(request.json, format_args!("followup_run_started={run_id}"));
            (
                if *from_conflict_choice {
                    "conflict_follow_up_started"
                } else {
                    "follow_up_started"
                },
                Some(*run_id),
            )
        },
        PatchApplyOutcome::ApprovedNotApplied {
            reason: NotAppliedReason::AbortRequired { .. },
        } => {
            progress(
                request.json,
                format_args!("conflict resolution requires aborting the current run before apply"),
            );
            ("abort_current_run_required", None)
        },
        PatchApplyOutcome::ApprovedNotApplied {
            reason: NotAppliedReason::TargetNotAtPickupBoundary,
        } => {
            progress(
                request.json,
                format_args!(
                    "approved patch stored; target run is not at an active pickup boundary"
                ),
            );
            ("approved_deferred", None)
        },
        PatchApplyOutcome::Rejected { .. } => ("rejected", None),
    };
    Ok(Applied {
        message: message.to_owned(),
        follow_up_run_id,
    })
}

/// Merge conflicts detected against the current roadmap into the draft, so
/// the approval prompt offers the right choices.
fn assess_for_approval(
    patch: RoadmapPatch,
    roadmap_path: &Path,
    roadmap_text: &str,
    json: bool,
) -> RoadmapPatch {
    let Ok(parsed) = parse_roadmap_document(roadmap_path, roadmap_text) else {
        tracing::warn!(
            target: "feature_cli",
            patch_id = %patch.id,
            roadmap_path = %roadmap_path.display(),
            "roadmap_patch_conflict_detection_parse_failed"
        );
        return patch;
    };
    let assessment = assess_patch(&patch, &parsed.roadmap);
    if let Some(issues) = &assessment.malformed {
        tracing::warn!(target: "feature_cli", patch_id = %patch.id, %issues, "roadmap_patch_apply_shape_invalid");
    }
    if !assessment.patch.conflicts.is_empty() {
        progress(
            json,
            format_args!(
                "conflicts={} choices={}",
                assessment.patch.conflicts.len(),
                conflict_choice_list(&assessment.patch.conflicts)
            ),
        );
    }
    assessment.patch
}

enum PatchDecision {
    Store {
        summary_hash: ContentHash,
    },
    Approve {
        conflict_choice: Option<OperatorConflictChoice>,
    },
    Reject {
        comment: Option<String>,
        conflict_choice: Option<OperatorConflictChoice>,
    },
}

impl PatchDecision {
    fn conflict_choice(&self) -> Option<OperatorConflictChoice> {
        match self {
            Self::Store { .. } => None,
            Self::Approve { conflict_choice }
            | Self::Reject {
                conflict_choice, ..
            } => *conflict_choice,
        }
    }
}

async fn decide_patch(
    writer: &surge_persistence::runs::RunWriter,
    patch: &RoadmapPatch,
    mode: FeatureApprovalMode,
    conflict_choice: Option<OperatorConflictChoice>,
) -> Result<PatchDecision> {
    validate_requested_conflict_choice(patch, conflict_choice)?;
    match mode {
        FeatureApprovalMode::Store => {
            let summary_hash = record_cli_approval_request(writer, patch).await?;
            Ok(PatchDecision::Store { summary_hash })
        },
        // The apply operation records the approval (or the rejection a
        // `reject_patch` choice means) and refuses an unresolved conflict.
        FeatureApprovalMode::Approve => Ok(PatchDecision::Approve { conflict_choice }),
        FeatureApprovalMode::Reject => {
            let conflict_choice = conflict_choice.or_else(|| {
                patch_has_conflicts(patch).then_some(OperatorConflictChoice::RejectPatch)
            });
            record_cli_decision(
                writer,
                patch,
                RoadmapPatchApprovalDecision::Reject,
                Some("rejected by --approval reject".into()),
                conflict_choice,
            )
            .await?;
            Ok(PatchDecision::Reject {
                comment: Some("rejected by --approval reject".into()),
                conflict_choice,
            })
        },
        FeatureApprovalMode::Prompt => prompt_for_patch_decision(writer, patch).await,
    }
}

async fn prompt_for_patch_decision(
    writer: &surge_persistence::runs::RunWriter,
    patch: &RoadmapPatch,
) -> Result<PatchDecision> {
    let mut approval_loop = RoadmapPatchApprovalLoop::new(0);
    let prompt = approval_loop
        .request_approval(writer, patch, local_approval_channel())
        .await?;
    println!("{}", prompt.summary);
    println!("[a] approve  [r] reject  [s] store pending");
    print!("choice: ");
    io::stdout().flush()?;

    let mut choice = String::new();
    io::stdin().read_line(&mut choice)?;
    match choice.trim().to_lowercase().as_str() {
        "" | "a" | "approve" => Ok(PatchDecision::Approve {
            conflict_choice: prompt_for_conflict_choice(patch)?,
        }),
        "r" | "reject" => {
            print!("reason: ");
            io::stdout().flush()?;
            let mut reason = String::new();
            io::stdin().read_line(&mut reason)?;
            let comment = reason.trim().to_owned();
            let comment = (!comment.is_empty()).then_some(comment);
            let conflict_choice =
                patch_has_conflicts(patch).then_some(OperatorConflictChoice::RejectPatch);
            approval_loop
                .record_decision(
                    writer,
                    &patch.id,
                    ApprovalChannelKind::Desktop,
                    RoadmapPatchApprovalResolution {
                        decision: RoadmapPatchApprovalDecision::Reject,
                        comment: comment.clone(),
                        conflict_choice,
                    },
                )
                .await?;
            Ok(PatchDecision::Reject {
                comment,
                conflict_choice,
            })
        },
        "s" | "store" | "pending" => Ok(PatchDecision::Store {
            summary_hash: prompt.summary_hash,
        }),
        other => Err(anyhow!("unknown approval choice: {other}")),
    }
}

async fn record_cli_decision(
    writer: &surge_persistence::runs::RunWriter,
    patch: &RoadmapPatch,
    decision: RoadmapPatchApprovalDecision,
    comment: Option<String>,
    conflict_choice: Option<OperatorConflictChoice>,
) -> Result<()> {
    let mut approval_loop = RoadmapPatchApprovalLoop::new(0);
    approval_loop
        .request_approval(writer, patch, local_approval_channel())
        .await?;
    approval_loop
        .record_decision(
            writer,
            &patch.id,
            ApprovalChannelKind::Desktop,
            RoadmapPatchApprovalResolution {
                decision,
                comment,
                conflict_choice,
            },
        )
        .await?;
    Ok(())
}

async fn record_cli_approval_request(
    writer: &surge_persistence::runs::RunWriter,
    patch: &RoadmapPatch,
) -> Result<ContentHash> {
    let prompt = RoadmapPatchApprovalLoop::new(0)
        .request_approval(writer, patch, local_approval_channel())
        .await?;
    Ok(prompt.summary_hash)
}

fn prompt_for_conflict_choice(patch: &RoadmapPatch) -> Result<Option<OperatorConflictChoice>> {
    let choices = patch_conflict_choices(patch);
    if choices.is_empty() {
        return Ok(None);
    }

    println!("conflict choices:");
    for (index, choice) in choices.iter().enumerate() {
        println!(
            "  [{}] {} - {}",
            index + 1,
            conflict_choice_label(*choice),
            conflict_choice_hint(*choice)
        );
    }
    print!("conflict choice: ");
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    parse_conflict_choice_input(input.trim(), &choices).map(Some)
}

fn parse_conflict_choice_input(
    input: &str,
    choices: &[OperatorConflictChoice],
) -> Result<OperatorConflictChoice> {
    if let Ok(index) = input.parse::<usize>()
        && (1..=choices.len()).contains(&index)
    {
        return Ok(choices[index - 1]);
    }
    choices
        .iter()
        .copied()
        .find(|choice| {
            conflict_choice_label(*choice) == input || clap_conflict_choice_label(*choice) == input
        })
        .ok_or_else(|| anyhow!("unknown conflict choice: {input}"))
}

fn validate_requested_conflict_choice(
    patch: &RoadmapPatch,
    choice: Option<OperatorConflictChoice>,
) -> Result<()> {
    let Some(choice) = choice else {
        return Ok(());
    };
    if patch_conflict_choices(patch).contains(&choice) {
        return Ok(());
    }
    Err(anyhow!(
        "conflict choice {} is not available for this patch",
        conflict_choice_label(choice)
    ))
}

fn patch_has_conflicts(patch: &RoadmapPatch) -> bool {
    !patch.conflicts.is_empty()
}

fn patch_conflict_choices(patch: &RoadmapPatch) -> Vec<OperatorConflictChoice> {
    let mut choices = Vec::new();
    for conflict in &patch.conflicts {
        choices.extend(conflict.choices.iter().copied());
    }
    choices.sort_by_key(|choice| conflict_choice_label(*choice));
    choices.dedup();
    choices
}

fn local_approval_channel() -> ApprovalChannel {
    ApprovalChannel::Desktop {
        duration: ApprovalDuration::Persistent,
    }
}

fn upsert_patch_index(
    storage: &Arc<Storage>,
    target: &RoadmapTargetCandidate,
    patch: &RoadmapPatch,
    owner_run_id: RunId,
    update: PatchIndexUpdate,
) -> Result<surge_persistence::roadmap_patches::RoadmapPatchIndexRecord> {
    storage
        .roadmap_patch_store()
        .upsert(&RoadmapPatchIndexUpsert {
            patch_id: patch.id.clone(),
            content_hash: update.content_hash,
            run_id: Some(owner_run_id),
            project_path: target.project_path.clone(),
            target: target.target.clone(),
            status: update.status,
            patch_artifact: update.patch_artifact,
            patch_path: update.patch_path,
            summary_hash: update.summary_hash,
            decision: update.decision,
            decision_comment: update.decision_comment,
            conflict_choice: update.conflict_choice,
            observed_at_ms: chrono::Utc::now().timestamp_millis(),
        })
        .map_err(Into::into)
}

struct PatchIndexUpdate {
    content_hash: ContentHash,
    status: RoadmapPatchStatus,
    patch_artifact: Option<ContentHash>,
    patch_path: Option<PathBuf>,
    summary_hash: Option<ContentHash>,
    decision: Option<RoadmapPatchApprovalDecision>,
    decision_comment: Option<String>,
    conflict_choice: Option<OperatorConflictChoice>,
}

fn normalize_patch_target(patch: &mut RoadmapPatch, target: &RoadmapTargetCandidate) {
    if patch.target != target.target {
        tracing::warn!(
            target: "feature_cli",
            patch_id = %patch.id,
            planner_target = ?patch.target,
            selected_target = ?target.target,
            "normalizing roadmap patch target to selected CLI target"
        );
        patch.target = target.target.clone();
    }
}

async fn append_feature_run_started(
    writer: &surge_persistence::runs::RunWriter,
    worktree: &Path,
) -> Result<()> {
    writer
        .append_event(VersionedEventPayload::new(EventPayload::RunStarted {
            pipeline_template: None,
            project_path: worktree.to_path_buf(),
            initial_prompt: String::new(),
            config: RunConfig {
                bootstrap_edit_loop_cap: None,
                budget: Default::default(),
                sandbox_default: SandboxMode::WorkspaceWrite,
                approval_default: ApprovalPolicy::OnRequest,
                auto_pr: false,
                mcp_servers: Vec::new(),
            },
        }))
        .await?;
    Ok(())
}

async fn append_feature_run_terminal(
    writer: &surge_persistence::runs::RunWriter,
    result: &Result<()>,
) -> Result<()> {
    let payload = match result {
        Ok(()) => EventPayload::RunCompleted {
            terminal_node: surge_core::keys::NodeKey::try_from(FEATURE_PLANNER_TERMINAL_NODE)
                .context("feature planner terminal node key")?,
        },
        Err(error) => EventPayload::RunFailed {
            error: error.to_string(),
        },
    };
    writer
        .append_event(VersionedEventPayload::new(payload))
        .await?;
    Ok(())
}

fn target_selector(args: &FeatureDescribeArgs) -> Result<RoadmapTargetSelector> {
    match (&args.run_id, args.project) {
        (Some(_), true) => Err(anyhow!("pass either --run <RUN_ID> or --project, not both")),
        (Some(run_id), false) => Ok(RoadmapTargetSelector::Run {
            run_id: parse_run_id(run_id)?,
        }),
        (None, true) => Ok(RoadmapTargetSelector::ProjectFile),
        (None, false) => Ok(RoadmapTargetSelector::Auto),
    }
}

fn resolve_feature_worktree(
    override_path: Option<&Path>,
    target: &RoadmapTargetCandidate,
    project_root: &Path,
) -> Result<PathBuf> {
    let worktree = override_path
        .map(Path::to_path_buf)
        .or_else(|| target.worktree_path.clone())
        .unwrap_or_else(|| project_root.to_path_buf());
    let worktree = absolute_path_from(project_root, worktree);
    if !worktree.exists() {
        return Err(anyhow!(
            "worktree path does not exist: {}",
            worktree.display()
        ));
    }
    Ok(worktree)
}

fn roadmap_prompt_text(path: &Path, content: &str) -> String {
    match parse_roadmap_document(path, content) {
        Ok(parsed) => {
            let mut prompt = if path.extension().and_then(|ext| ext.to_str()) == Some("toml") {
                parsed.roadmap.to_markdown()
            } else {
                content.to_owned()
            };
            prompt.push_str(&roadmap_identifiers_prompt(&parsed));
            prompt
        },
        Err(_) => content.to_owned(),
    }
}

fn target_label(target: &RoadmapTargetCandidate) -> String {
    match target.run_id {
        Some(run_id) => format!(
            "run:{run_id}:{}",
            target
                .run_status
                .map_or_else(|| "unknown".to_owned(), |status| status.to_string())
        ),
        None => format!("project:{}", target.roadmap_path.display()),
    }
}

fn print_patch_table(records: &[RoadmapPatchIndexRecord]) {
    println!(
        "{:<24} {:<16} {:<28} UPDATED",
        "PATCH_ID", "STATUS", "TARGET"
    );
    for record in records {
        println!(
            "{:<24} {:<16} {:<28} {}",
            record.patch_id,
            patch_status_label(record.status),
            patch_target_label(&record.target),
            record.updated_at_ms
        );
    }
}

fn print_patch_detail(record: &RoadmapPatchIndexRecord) {
    println!("patch_id={}", record.patch_id);
    println!("status={}", patch_status_label(record.status));
    println!("target={}", patch_target_label(&record.target));
    println!("content_hash={}", record.content_hash);
    if let Some(run_id) = record.run_id {
        println!("run_id={run_id}");
    }
    if let Some(path) = &record.patch_path {
        println!("patch_path={}", path.display());
    }
    if let Some(hash) = record.patch_artifact {
        println!("patch_artifact={hash}");
    }
    if let Some(decision) = record.decision {
        println!("decision={}", approval_decision_label(decision));
    }
    if let Some(comment) = &record.decision_comment {
        println!("comment={comment}");
    }
    if let Some(choice) = record.conflict_choice {
        println!("conflict_choice={}", conflict_choice_label(choice));
    }
    println!("created_at_ms={}", record.created_at_ms);
    println!("updated_at_ms={}", record.updated_at_ms);
}

fn patch_target_label(target: &RoadmapPatchTarget) -> String {
    match target {
        RoadmapPatchTarget::ProjectRoadmap { roadmap_path } => {
            format!("project:{roadmap_path}")
        },
        RoadmapPatchTarget::RunRoadmap { run_id, .. } => format!("run:{run_id}"),
    }
}

fn parse_run_id(value: &str) -> Result<RunId> {
    value
        .parse()
        .map_err(|error| anyhow!("invalid run id '{value}': {error}"))
}

fn parse_patch_id(value: &str) -> Result<RoadmapPatchId> {
    RoadmapPatchId::new(value)
        .map_err(|error| anyhow!("invalid roadmap patch id '{value}': {error}"))
}

fn output_message(status: RoadmapPatchStatus) -> String {
    match status {
        RoadmapPatchStatus::Drafted => "patch drafted".into(),
        RoadmapPatchStatus::PendingApproval => "patch stored pending approval".into(),
        RoadmapPatchStatus::Approved => "patch approved and stored".into(),
        RoadmapPatchStatus::Applied => "patch applied".into(),
        RoadmapPatchStatus::Rejected => "patch rejected".into(),
        RoadmapPatchStatus::Superseded => "patch superseded".into(),
    }
}

fn patch_status_label(status: RoadmapPatchStatus) -> &'static str {
    match status {
        RoadmapPatchStatus::Drafted => "drafted",
        RoadmapPatchStatus::PendingApproval => "pending_approval",
        RoadmapPatchStatus::Approved => "approved",
        RoadmapPatchStatus::Applied => "applied",
        RoadmapPatchStatus::Rejected => "rejected",
        RoadmapPatchStatus::Superseded => "superseded",
    }
}

fn approval_decision_label(decision: RoadmapPatchApprovalDecision) -> &'static str {
    match decision {
        RoadmapPatchApprovalDecision::Approve => "approve",
        RoadmapPatchApprovalDecision::Edit => "edit",
        RoadmapPatchApprovalDecision::Reject => "reject",
    }
}

fn conflict_choice_label(choice: OperatorConflictChoice) -> &'static str {
    match choice {
        OperatorConflictChoice::DeferToNextMilestone => "defer_to_next_milestone",
        OperatorConflictChoice::AbortCurrentRun => "abort_current_run",
        OperatorConflictChoice::CreateFollowUpRun => "create_follow_up_run",
        OperatorConflictChoice::RejectPatch => "reject_patch",
    }
}

fn clap_conflict_choice_label(choice: OperatorConflictChoice) -> &'static str {
    match choice {
        OperatorConflictChoice::DeferToNextMilestone => "defer-to-next-milestone",
        OperatorConflictChoice::AbortCurrentRun => "abort-current-run",
        OperatorConflictChoice::CreateFollowUpRun => "create-follow-up-run",
        OperatorConflictChoice::RejectPatch => "reject-patch",
    }
}

fn conflict_choice_hint(choice: OperatorConflictChoice) -> &'static str {
    match choice {
        OperatorConflictChoice::DeferToNextMilestone => "move work to the next pending milestone",
        OperatorConflictChoice::AbortCurrentRun => "pause here and abort the active run first",
        OperatorConflictChoice::CreateFollowUpRun => "materialize a separate follow-up run",
        OperatorConflictChoice::RejectPatch => "reject this roadmap patch",
    }
}

fn conflict_choice_list(conflicts: &[RoadmapPatchConflict]) -> String {
    let mut choices = Vec::new();
    for conflict in conflicts {
        choices.extend(conflict.choices.iter().copied());
    }
    choices.sort_by_key(|choice| conflict_choice_label(*choice));
    choices.dedup();
    choices
        .into_iter()
        .map(conflict_choice_label)
        .collect::<Vec<_>>()
        .join(",")
}

fn progress(json: bool, args: std::fmt::Arguments<'_>) {
    if json {
        eprintln!("{args}");
    } else {
        println!("{args}");
    }
}

fn emit_output(json: bool, output: FeatureDescribeOutput) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        println!("{}", output.message);
        if let Some(patch_id) = &output.patch_id {
            println!("patch_id={patch_id}");
        }
        println!("planner_run_id={}", output.planner_run_id);
        if let Some(run_id) = &output.follow_up_run_id {
            println!("followup_run_id={run_id}");
        }
        if let Some(choice) = &output.conflict_choice {
            println!("conflict_choice={choice}");
        }
    }
    Ok(())
}

#[derive(Serialize)]
struct FeatureDescribeOutput {
    status: String,
    planner_run_id: String,
    patch_id: Option<String>,
    target: String,
    patch_artifact: Option<String>,
    patch_path: Option<String>,
    follow_up_run_id: Option<String>,
    conflict_choice: Option<String>,
    message: String,
}

impl FeatureDescribeOutput {
    fn out_of_scope(planner_run_id: RunId, target: &RoadmapTargetCandidate) -> Self {
        Self {
            status: "out_of_scope".into(),
            planner_run_id: planner_run_id.to_string(),
            patch_id: None,
            target: target_label(target),
            patch_artifact: None,
            patch_path: None,
            follow_up_run_id: None,
            conflict_choice: None,
            message: "request is out of scope for the selected roadmap".into(),
        }
    }
}

fn load_project_config_for_current_repo() -> Result<(SurgeConfig, PathBuf)> {
    let project_root = GitManager::discover()
        .map(|manager| manager.repo_path().to_path_buf())
        .unwrap_or_else(|_| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    Ok((load_project_config_at(&project_root)?, project_root))
}

fn load_project_config_at(project_root: &Path) -> Result<SurgeConfig> {
    let config_path = project_root.join("surge.toml");
    if config_path.exists() {
        SurgeConfig::load(&config_path).with_context(|| format!("load {}", config_path.display()))
    } else {
        SurgeConfig::discover().context("load surge config")
    }
}

#[cfg(windows)]
fn surge_home_dir() -> Result<PathBuf> {
    super::common::surge_home_dir()
}

#[cfg(not(windows))]
fn surge_home_dir() -> Result<PathBuf> {
    // Honour the SURGE_HOME env var first so tests can isolate per-tempdir.
    // `dirs::home_dir()` on Windows uses `SHGetKnownFolderPath` which ignores
    // `USERPROFILE` overrides, so falling back to it directly makes parallel
    // tests on Windows race on a single real `~/.surge/` SQLite file.
    let dir = if let Ok(custom) = std::env::var("SURGE_HOME") {
        if !custom.is_empty() {
            PathBuf::from(custom)
        } else {
            dirs::home_dir()
                .ok_or_else(|| anyhow!("SURGE_HOME unset and home directory unknown"))?
                .join(".surge")
        }
    } else {
        dirs::home_dir()
            .ok_or_else(|| anyhow!("SURGE_HOME unset and home directory unknown"))?
            .join(".surge")
    };
    std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    Ok(dir)
}

fn absolute_path_from(base: &Path, path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        base.join(path)
    }
}
