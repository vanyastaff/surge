//! Roadmap amendment target discovery.
//!
//! This module normalizes "what roadmap should this patch amend?" before the
//! CLI or notification layers get involved. It returns typed candidates with
//! artifact hashes and pickup policy instead of asking callers to infer target
//! semantics from paths and run status strings.

use std::path::PathBuf;
use std::sync::Arc;

use surge_core::roadmap_patch::{ActivePickupPolicy, RoadmapPatchTarget};
use surge_core::{ContentHash, RunId, RunStatus};
use surge_persistence::runs::{
    ArtifactRecord, OpenError, RunFilter, RunSummary, Storage, StorageError,
};

const ROADMAP_ARTIFACT_NAMES: &[&str] = &[
    "roadmap",
    "roadmap.md",
    "roadmap.toml",
    "roadmap-md",
    "roadmap-toml",
];
const FLOW_ARTIFACT_NAMES: &[&str] = &["flow", "flow.toml", "flow-toml"];

/// Selector supplied by CLI or higher-level orchestration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoadmapTargetSelector {
    /// Pick automatically when there is exactly one viable candidate.
    Auto,
    /// Amend the project-level roadmap file.
    ProjectFile,
    /// Amend the roadmap artifact attached to a specific run.
    Run {
        /// Target run id.
        run_id: RunId,
    },
}

/// Where it is safe to apply the selected amendment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoadmapAmendmentPoint {
    /// Direct project roadmap file amendment.
    ProjectFile,
    /// Active run can observe the update at a future safe loop boundary.
    ActiveRunBoundary,
    /// Terminal or non-pickup run requires a follow-up run.
    FollowUpRun,
    /// Target exists but active pickup is not currently safe.
    Deferred,
}

/// Typed roadmap target candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoadmapTargetCandidate {
    /// Selector that reproduces this candidate explicitly.
    pub selector: RoadmapTargetSelector,
    /// Patch target to embed in `roadmap-patch.toml`.
    pub target: RoadmapPatchTarget,
    /// Project path associated with this target.
    pub project_path: PathBuf,
    /// Current roadmap content hash.
    pub roadmap_hash: Option<ContentHash>,
    /// Current roadmap path.
    pub roadmap_path: PathBuf,
    /// Current flow content hash, when available.
    pub flow_hash: Option<ContentHash>,
    /// Current flow artifact path, when available.
    pub flow_path: Option<PathBuf>,
    /// Owning run id, when this candidate is run-backed.
    pub run_id: Option<RunId>,
    /// Owning run status, when this candidate is run-backed.
    pub run_status: Option<RunStatus>,
    /// Run worktree path, when this candidate is run-backed.
    pub worktree_path: Option<PathBuf>,
    /// Safe amendment point implied by run status and pickup policy.
    pub amendment_point: RoadmapAmendmentPoint,
    /// Whether an active runner may pick up the amendment.
    pub active_pickup: ActivePickupPolicy,
}

/// Short candidate label used in ambiguity errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoadmapTargetSummary {
    /// Explicit selector for this candidate.
    pub selector: RoadmapTargetSelector,
    /// Human-readable label.
    pub label: String,
}

/// Errors returned by [`RoadmapTargetResolver`].
#[derive(Debug, thiserror::Error)]
pub enum RoadmapTargetError {
    /// Project roadmap path does not exist.
    #[error("project roadmap not found: {0}")]
    ProjectRoadmapMissing(PathBuf),
    /// Requested run is absent from the registry.
    #[error("run not found: {0}")]
    RunNotFound(RunId),
    /// Requested run has no roadmap artifact in its materialized view.
    #[error("run {0} has no roadmap artifact")]
    RunRoadmapMissing(RunId),
    /// Auto-selection found no viable candidates.
    #[error("no roadmap amendment target found")]
    NoTarget,
    /// Auto-selection found multiple viable candidates.
    #[error("ambiguous roadmap amendment target ({count} candidates)")]
    Ambiguous {
        /// Number of candidates.
        count: usize,
        /// Candidate labels for UI/CLI prompts.
        candidates: Vec<RoadmapTargetSummary>,
    },
    /// Opening a run reader failed.
    #[error("open run reader failed: {0}")]
    Open(#[from] OpenError),
    /// Reading storage failed.
    #[error("storage failed: {0}")]
    Storage(#[from] StorageError),
    /// Reading a project roadmap file failed.
    #[error("I/O failed: {0}")]
    Io(#[from] std::io::Error),
}

/// Discovers project-file and run-backed roadmap amendment targets.
#[derive(Clone)]
pub struct RoadmapTargetResolver {
    storage: Arc<Storage>,
    project_path: PathBuf,
    project_roadmap_path: PathBuf,
}

impl RoadmapTargetResolver {
    /// Create a resolver for one project.
    #[must_use]
    pub fn new(
        storage: Arc<Storage>,
        project_path: impl Into<PathBuf>,
        project_roadmap_path: impl Into<PathBuf>,
    ) -> Self {
        Self {
            storage,
            project_path: project_path.into(),
            project_roadmap_path: project_roadmap_path.into(),
        }
    }

    /// Resolve one selector to a concrete candidate.
    ///
    /// # Errors
    /// Returns [`RoadmapTargetError`] when the target is absent, ambiguous, or
    /// cannot be read from storage.
    pub async fn resolve(
        &self,
        selector: RoadmapTargetSelector,
    ) -> Result<RoadmapTargetCandidate, RoadmapTargetError> {
        tracing::debug!(
            target: "roadmap_target",
            selector = ?selector,
            project_path = %self.project_path.display(),
            "roadmap_target_resolve"
        );
        match selector {
            RoadmapTargetSelector::ProjectFile => self.project_candidate_required().await,
            RoadmapTargetSelector::Run { run_id } => self.run_candidate_required(run_id).await,
            RoadmapTargetSelector::Auto => self.resolve_auto().await,
        }
    }

    /// Discover every viable candidate for this project.
    ///
    /// # Errors
    /// Returns [`RoadmapTargetError`] when run storage cannot be read.
    pub async fn discover(&self) -> Result<Vec<RoadmapTargetCandidate>, RoadmapTargetError> {
        let mut candidates = Vec::new();
        if let Some(project) = self.project_candidate_optional().await? {
            candidates.push(project);
        }

        let runs = self
            .storage
            .list_runs(RunFilter {
                project_path: Some(self.project_path.clone()),
                ..RunFilter::default()
            })
            .await?;
        for run in runs {
            if let Some(candidate) = self.run_candidate_from_summary(&run).await? {
                candidates.push(candidate);
            }
        }
        Ok(candidates)
    }

    async fn resolve_auto(&self) -> Result<RoadmapTargetCandidate, RoadmapTargetError> {
        let candidates = self.discover().await?;
        match candidates.len() {
            0 => Err(RoadmapTargetError::NoTarget),
            1 => {
                let mut candidates = candidates.into_iter();
                candidates.next().ok_or(RoadmapTargetError::NoTarget)
            },
            count => Err(RoadmapTargetError::Ambiguous {
                count,
                candidates: candidates.iter().map(candidate_summary).collect(),
            }),
        }
    }

    async fn project_candidate_required(
        &self,
    ) -> Result<RoadmapTargetCandidate, RoadmapTargetError> {
        self.project_candidate_optional()
            .await?
            .ok_or_else(|| RoadmapTargetError::ProjectRoadmapMissing(self.project_roadmap_abs()))
    }

    async fn project_candidate_optional(
        &self,
    ) -> Result<Option<RoadmapTargetCandidate>, RoadmapTargetError> {
        let path = self.project_roadmap_abs();
        let bytes = match tokio::fs::read(&path).await {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let hash = ContentHash::compute(&bytes);
        tracing::info!(
            target: "roadmap_target",
            roadmap_path = %path.display(),
            roadmap_hash = %hash,
            "project_roadmap_target_discovered"
        );
        Ok(Some(RoadmapTargetCandidate {
            selector: RoadmapTargetSelector::ProjectFile,
            target: RoadmapPatchTarget::ProjectRoadmap {
                roadmap_path: self.project_roadmap_target_path(),
            },
            project_path: self.project_path.clone(),
            roadmap_hash: Some(hash),
            roadmap_path: path,
            flow_hash: None,
            flow_path: None,
            run_id: None,
            run_status: None,
            worktree_path: None,
            amendment_point: RoadmapAmendmentPoint::ProjectFile,
            active_pickup: ActivePickupPolicy::FollowUpOnly,
        }))
    }

    async fn run_candidate_required(
        &self,
        run_id: RunId,
    ) -> Result<RoadmapTargetCandidate, RoadmapTargetError> {
        let Some(summary) = self.storage.get_run(&run_id).await? else {
            return Err(RoadmapTargetError::RunNotFound(run_id));
        };
        self.run_candidate_from_summary(&summary)
            .await?
            .ok_or(RoadmapTargetError::RunRoadmapMissing(run_id))
    }

    async fn run_candidate_from_summary(
        &self,
        summary: &RunSummary,
    ) -> Result<Option<RoadmapTargetCandidate>, RoadmapTargetError> {
        let reader = self.storage.open_run_reader(summary.id).await?;
        let artifacts = reader.artifacts().await?;
        let Some(roadmap) = latest_named_artifact(&artifacts, ROADMAP_ARTIFACT_NAMES) else {
            return Ok(None);
        };
        let flow = latest_named_artifact(&artifacts, FLOW_ARTIFACT_NAMES);
        let active_pickup = pickup_for_status(summary.status);
        tracing::info!(
            target: "roadmap_target",
            run_id = %summary.id,
            run_status = summary.status.as_str(),
            roadmap_hash = %roadmap.content_hash,
            active_pickup = ?active_pickup,
            "run_roadmap_target_discovered"
        );
        Ok(Some(RoadmapTargetCandidate {
            selector: RoadmapTargetSelector::Run { run_id: summary.id },
            target: RoadmapPatchTarget::RunRoadmap {
                run_id: summary.id,
                roadmap_artifact: Some(roadmap.content_hash),
                flow_artifact: flow.map(|artifact| artifact.content_hash),
                active_pickup,
            },
            project_path: summary.project_path.clone(),
            roadmap_hash: Some(roadmap.content_hash),
            roadmap_path: roadmap.path.clone(),
            flow_hash: flow.map(|artifact| artifact.content_hash),
            flow_path: flow.map(|artifact| artifact.path.clone()),
            run_id: Some(summary.id),
            run_status: Some(summary.status),
            worktree_path: Some(reader.worktree_path().to_path_buf()),
            amendment_point: amendment_point_for_status(summary.status),
            active_pickup,
        }))
    }

    fn project_roadmap_abs(&self) -> PathBuf {
        if self.project_roadmap_path.is_absolute() {
            self.project_roadmap_path.clone()
        } else {
            self.project_path.join(&self.project_roadmap_path)
        }
    }

    fn project_roadmap_target_path(&self) -> String {
        let path = if self.project_roadmap_path.is_absolute() {
            self.project_roadmap_path
                .strip_prefix(&self.project_path)
                .unwrap_or(&self.project_roadmap_path)
        } else {
            &self.project_roadmap_path
        };
        path.to_string_lossy().replace('\\', "/")
    }
}

fn latest_named_artifact<'a>(
    artifacts: &'a [ArtifactRecord],
    names: &[&str],
) -> Option<&'a ArtifactRecord> {
    artifacts
        .iter()
        .filter(|artifact| names.contains(&artifact.name.as_str()))
        .max_by_key(|artifact| artifact.produced_at_seq)
}

fn pickup_for_status(status: RunStatus) -> ActivePickupPolicy {
    match status {
        RunStatus::Running => ActivePickupPolicy::Allowed,
        RunStatus::Bootstrapping => ActivePickupPolicy::Disabled,
        RunStatus::Completed | RunStatus::Failed | RunStatus::Aborted | RunStatus::Crashed => {
            ActivePickupPolicy::FollowUpOnly
        },
    }
}

fn amendment_point_for_status(status: RunStatus) -> RoadmapAmendmentPoint {
    match status {
        RunStatus::Running => RoadmapAmendmentPoint::ActiveRunBoundary,
        RunStatus::Bootstrapping => RoadmapAmendmentPoint::Deferred,
        RunStatus::Completed | RunStatus::Failed | RunStatus::Aborted | RunStatus::Crashed => {
            RoadmapAmendmentPoint::FollowUpRun
        },
    }
}

fn candidate_summary(candidate: &RoadmapTargetCandidate) -> RoadmapTargetSummary {
    let label = match (candidate.run_id, candidate.run_status) {
        (Some(run_id), Some(status)) => format!("run {run_id} ({status})"),
        _ => format!("project file {}", candidate.roadmap_path.display()),
    };
    RoadmapTargetSummary {
        selector: candidate.selector.clone(),
        label,
    }
}
