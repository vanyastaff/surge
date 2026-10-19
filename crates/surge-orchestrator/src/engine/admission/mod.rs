//! Run admission (#97): the one producer of an admitted run.
//!
//! Before a run executes a single stage it is *admitted*. An admission input
//! is one of three kinds ([`AdmissionKind`]):
//!
//! - **fresh**: a new untracked run (graph, worktree and config from the
//!   caller);
//! - **replayed**: an existing untracked run resumed from its event log;
//! - **task-claimed**: a durable task reservation started or resumed under a
//!   launch claim. Graph, workspace, task context and frozen config come from
//!   the reservation, never from the caller.
//!
//! Admission returns an [`Admitted`] run or a typed [`AdmissionRejection`].
//! `Engine` keeps only the active-run registry and the spawning of the run
//! task.
//!
//! # Ordering invariants
//! 1. The launch claim is validated first; an unclaimed admission of a
//!    reserved task run is refused as [`AdmissionRejection::OwnershipRequired`].
//! 2. "Already active" is checked before any durable mutation.
//! 3. On resume, prior MCP process groups are stopped and assessed (ADR-0021)
//!    before any registry change.
//! 4. Validation runs before the writer is created or opened, and before a
//!    resume clears its Parked row or appends `RunWokeFromPark`: a rejected
//!    admission changes nothing durable.
//! 5. Clearing a parked run's registry row is mandatory; `RunWokeFromPark` is
//!    appended best-effort after the writer opens (ADR-0016 M3/M4).
//! 6. A fresh start requires its worktree to exist.
//! 7. Every rejection after the writer opens closes it.
//!
//! # Validation sequence
//! One sequence ([`ValidationRule`]), with an explicit rule for which steps
//! apply to which kind. A task-claimed start does **not** re-run it: its
//! config was frozen and validated by the same sequence at reservation time
//! ([`Admission::freeze_task_config`]). Resumes validate the graph against the
//! *current* profile registry, plus the quota policy, and never rewrite the
//! frozen config.

mod startup;
#[cfg(test)]
mod tests;

pub use startup::MEMORY_CLAIM_CANDIDATES_ARTIFACT_NAME;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use surge_core::graph::Graph;
use surge_core::id::RunId;
use surge_core::mcp_config::McpServerRef;
use surge_core::run_event::{EventPayload, VersionedEventPayload};
use surge_persistence::artifacts::ArtifactStore;
use surge_persistence::runs::Storage;
use surge_persistence::runs::run_writer::RunWriter;
use surge_persistence::work_items::WorkItemLaunchClaim;

use crate::engine::config::{EngineConfig, EngineRunConfig, RunSeedArtifact};
use crate::engine::engine::ActiveRun;
use crate::engine::error::EngineError;
use crate::engine::execution_state::{self, ExecutionState, Position};
use crate::engine::handle::RunOutcome;
use crate::engine::owned_effects::OwnedFlowEffectFence;
use crate::engine::quota_recovery::{ContinueEvidence, ResumeEvidence};

/// What is being admitted.
pub(crate) enum AdmissionKind<'c> {
    /// A new untracked run.
    Fresh {
        /// The run's identity.
        run_id: RunId,
        /// The graph to execute.
        graph: Box<Graph>,
        /// The run's worktree.
        worktree: PathBuf,
        /// The caller's run configuration.
        config: Box<EngineRunConfig>,
    },
    /// An existing untracked run resumed from its event log.
    Replayed {
        /// The run's identity.
        run_id: RunId,
        /// The run's worktree.
        worktree: PathBuf,
    },
    /// Host-owned replay with a captured project catalog.
    ScopedReplay {
        run_id: RunId,
        worktree: PathBuf,
        catalog: Arc<surge_acp::Registry>,
    },
    /// Start a durable task reservation under its launch claim.
    TaskStart(&'c WorkItemLaunchClaim),
    /// Resume a durable task run under its launch claim.
    TaskResume(&'c WorkItemLaunchClaim),
}

/// Why a run was not admitted. Closed: every caller handles every refusal.
#[derive(Debug, thiserror::Error)]
pub(crate) enum AdmissionRejection {
    /// An untracked admission of a reserved task run, or a task admission
    /// without a valid launch claim.
    #[error("{0}")]
    OwnershipRequired(String),
    /// A suspended task without a Continue reservation for the claim's
    /// generation.
    #[error("suspended task requires its durable Continue reservation for generation {generation}")]
    StaleContinue {
        /// The claim's attempt generation.
        generation: u64,
    },
    /// The run is already executing in this process.
    #[error("run is already active in this process: {0}")]
    AlreadyActive(RunId),
    /// The run's worktree does not exist.
    #[error("worktree path does not exist: {}", .0.display())]
    WorktreeMissing(PathBuf),
    /// The graph (or its loop seeds) failed validation at start.
    #[error(transparent)]
    GraphInvalid(Box<EngineError>),
    /// A resumed run's graph no longer validates against the current profile
    /// registry (for example a referenced profile was removed while it was
    /// parked). Operator-fixable: never a reason to fail the run.
    #[error("resume refused: the run's graph no longer validates: {0}")]
    ResumeGraphInvalid(Box<EngineError>),
    /// The quota policy is invalid for the graph.
    #[error(transparent)]
    QuotaPolicyInvalid(Box<EngineError>),
    /// A prior MCP process group of this run could not be stopped or its
    /// ownership is unclear (ADR-0021).
    #[error("{0}")]
    McpGroupNotStopped(String),
    /// A task start found an existing non-empty journal.
    #[error("task start refuses nonempty existing journal")]
    NonEmptyJournal,
    /// Owned-Flow inputs are missing, foreign, or forbidden by the current
    /// control.
    #[error("{0}")]
    OwnedFlowInputs(String),
    /// A follow-up run's bootstrap handoff was refused (#98).
    #[error("bootstrap handoff refused: {0}")]
    BootstrapHandoff(crate::bootstrap::handoff::HandoffRejection),
    /// The log already claims termination more than once (#95).
    #[error("run {run_id} has conflicting terminal history at seqs {seqs:?}")]
    TerminalConflict {
        /// The run.
        run_id: RunId,
        /// The conflicting terminal claims' sequences.
        seqs: Vec<u64>,
    },
    /// An actual storage fault.
    #[error("storage error: {0}")]
    Storage(String),
    /// A non-storage failure while preparing the run (artifact rendering,
    /// startup event construction).
    #[error(transparent)]
    Internal(Box<EngineError>),
}

impl From<AdmissionRejection> for EngineError {
    fn from(rejection: AdmissionRejection) -> Self {
        match rejection {
            AdmissionRejection::OwnershipRequired(reason) => Self::OwnershipRequired(reason),
            AdmissionRejection::StaleContinue { generation } => Self::StaleContinue { generation },
            AdmissionRejection::AlreadyActive(run_id) => Self::RunAlreadyActive(run_id),
            AdmissionRejection::WorktreeMissing(path) => Self::WorktreeMissing(path),
            AdmissionRejection::GraphInvalid(error)
            | AdmissionRejection::QuotaPolicyInvalid(error)
            | AdmissionRejection::Internal(error) => *error,
            AdmissionRejection::ResumeGraphInvalid(error) => {
                Self::ResumeGraphInvalid(error.to_string())
            },
            AdmissionRejection::McpGroupNotStopped(diagnostic) => {
                Self::McpGroupNotStopped(diagnostic)
            },
            AdmissionRejection::NonEmptyJournal => {
                Self::Storage("task start refuses nonempty existing journal".into())
            },
            AdmissionRejection::OwnedFlowInputs(reason) => Self::WorkItemRejected(reason),
            AdmissionRejection::TerminalConflict { run_id, seqs } => {
                Self::TerminalHistoryConflict { run_id, seqs }
            },
            AdmissionRejection::Storage(message) => Self::Storage(message),
            AdmissionRejection::BootstrapHandoff(rejection) => Self::BootstrapHandoff(rejection),
        }
    }
}

fn storage<E: std::fmt::Display>(error: E) -> AdmissionRejection {
    AdmissionRejection::Storage(error.to_string())
}

/// Admission's result.
pub(crate) enum Admitted {
    /// Ready to execute.
    Run(Box<AdmittedRun>),
    /// The log already ends in a single terminal event: settle the open
    /// writer with this outcome, never execute.
    Terminal {
        /// The open writer.
        writer: RunWriter,
        /// The log's terminal outcome.
        outcome: RunOutcome,
    },
}

/// Everything the run task needs, validated.
pub(crate) struct AdmittedRun {
    /// The run's identity.
    pub run_id: RunId,
    /// Where execution stands (#100): the latest execution state, loaded
    /// right after the startup prefix (fresh) or from the log (resume). Its
    /// graph is the graph to execute.
    pub execution: ExecutionState,
    /// The run's worktree.
    pub worktree_path: PathBuf,
    /// The validated, frozen run configuration.
    pub run_config: EngineRunConfig,
    /// The open run writer.
    pub writer: RunWriter,
    /// The run's artifact store.
    pub artifact_store: ArtifactStore,
    /// The launch claim of a task-claimed run.
    pub claim: Option<WorkItemLaunchClaim>,
    /// What a resume found, for the capacity bypass rule; `None` for a
    /// fresh start.
    pub resume_evidence: Option<ResumeEvidence>,
    /// The owned-flow effect fence, for owned-Flow task runs.
    pub effect_fence: Option<Arc<OwnedFlowEffectFence>>,
    /// The per-run MCP registry.
    pub mcp_registry: Option<Arc<surge_mcp::McpRegistry>>,
    /// The MCP servers the run actually exposes.
    pub mcp_servers: Vec<McpServerRef>,
    /// The run's agent catalog.
    pub agent_registry: Option<Arc<surge_acp::Registry>>,
    /// The engine-wide capacity ledger.
    pub capacity_ledger: Arc<dyn crate::engine::capacity::CapacityLedger>,
    /// The run's work estimator.
    pub capacity_estimator: Arc<dyn crate::engine::capacity::WorkEstimator>,
}

/// Which validation steps apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ValidationRule {
    /// A new untracked run.
    Fresh,
    /// Freezing a task reservation's config.
    Freeze,
    /// Resuming (replayed or task-claimed) against the current registry.
    Resume,
}

/// Run admission over one engine's storage and configuration.
pub(crate) struct Admission<'e> {
    /// The engine's storage.
    pub storage: &'e Arc<Storage>,
    /// The engine's configuration.
    pub config: &'e EngineConfig,
    /// The engine-wide MCP registry fallback.
    pub mcp_registry: Option<&'e Arc<surge_mcp::McpRegistry>>,
    /// The engine's active runs.
    pub active_runs: &'e tokio::sync::RwLock<HashMap<RunId, ActiveRun>>,
}

impl Admission<'_> {
    /// Admit one run.
    ///
    /// # Errors
    /// A typed [`AdmissionRejection`]; nothing durable changed unless the
    /// rejection happened after the writer opened, in which case the writer
    /// is closed.
    pub(crate) async fn admit(
        &self,
        kind: AdmissionKind<'_>,
    ) -> Result<Admitted, AdmissionRejection> {
        match kind {
            AdmissionKind::Fresh {
                run_id,
                graph,
                worktree,
                config,
            } => self.start(run_id, *graph, worktree, *config, None).await,
            AdmissionKind::TaskStart(claim) => {
                self.storage
                    .work_items()
                    .validate_claim(claim)
                    .map_err(|error| AdmissionRejection::OwnershipRequired(error.to_string()))?;
                let (attempt, workspace, _, config) =
                    crate::engine::work_items::pinned(self.storage, claim).map_err(engine)?;
                self.start(
                    attempt.run,
                    *attempt.graph,
                    workspace.path,
                    config,
                    Some(claim),
                )
                .await
            },
            AdmissionKind::Replayed { run_id, worktree } => {
                self.refuse_reserved_task(run_id, "task resume requires host ownership")?;
                self.resume(run_id, worktree, None, None, None).await
            },
            AdmissionKind::ScopedReplay {
                run_id,
                worktree,
                catalog,
            } => {
                self.refuse_reserved_task(run_id, "task resume requires host ownership")?;
                self.resume(run_id, worktree, None, None, Some(catalog))
                    .await
            },
            AdmissionKind::TaskResume(claim) => {
                let attempt = self
                    .storage
                    .work_items()
                    .validate_claim(claim)
                    .map_err(|error| AdmissionRejection::OwnershipRequired(error.to_string()))?;
                let generation = claim.binding().generation;
                // The Continue control, read once, after the claim is valid.
                let control = self
                    .storage
                    .work_items()
                    .execution_control(claim.run())
                    .map_err(storage)?;
                let continue_evidence = control.as_ref().map(|control| ContinueEvidence {
                    reserved: control.state
                        == surge_core::execution_recovery::ExecutionControlState::ContinueReserved,
                    generation: control.attempt_generation,
                    capacity_reason: control.fence.as_ref().is_some_and(|fence| {
                        matches!(
                            fence.reason,
                            surge_core::execution_recovery::SuspensionReason::Capacity { .. }
                        )
                    }),
                    claim_generation: generation,
                });
                if attempt.state == surge_core::work_item::WorkItemAttemptState::Suspended
                    && !continue_evidence
                        .as_ref()
                        .is_some_and(|control| control.reserved && control.generation == generation)
                {
                    return Err(AdmissionRejection::StaleContinue { generation });
                }
                let worktree = crate::engine::work_items::validate_resume(self.storage, claim)
                    .await
                    .map_err(engine)?;
                self.resume(claim.run(), worktree, Some(claim), continue_evidence, None)
                    .await
            },
        }
    }

    /// Freeze and validate a task reservation's config: the task-claimed
    /// front half of admission, run once at reservation time.
    ///
    /// # Errors
    /// A typed [`AdmissionRejection`]; nothing durable is written.
    pub(crate) async fn freeze_task_config(
        &self,
        graph: &Graph,
        mut config: EngineRunConfig,
        working_dir: &Path,
    ) -> Result<EngineRunConfig, AdmissionRejection> {
        self.validate(
            graph,
            &mut config,
            ValidationRule::Freeze,
            Some(working_dir),
        )
        .await?;
        Ok(config)
    }

    /// The agent catalog a run launches with: its own when the launcher
    /// supplied one, else the engine-wide catalog.
    pub(crate) fn agent_registry_for(
        &self,
        run_config: &EngineRunConfig,
    ) -> Option<Arc<surge_acp::Registry>> {
        run_config
            .agent_registry
            .clone()
            .or_else(|| self.config.agent_registry.clone())
    }

    /// The one validation sequence, with its per-kind rule.
    async fn validate(
        &self,
        graph: &Graph,
        config: &mut EngineRunConfig,
        rule: ValidationRule,
        working_dir: Option<&Path>,
    ) -> Result<(), AdmissionRejection> {
        use crate::engine::validate::{
            validate_graph_against, validate_loop_seeds, validate_quota_policy,
        };
        let graph_rejection = |error: EngineError| match rule {
            ValidationRule::Resume => AdmissionRejection::ResumeGraphInvalid(Box::new(error)),
            ValidationRule::Fresh | ValidationRule::Freeze => {
                AdmissionRejection::GraphInvalid(Box::new(error))
            },
        };
        // A profile registry lets validation resolve profile references and
        // agent-runtime identity (`ProfileNotFound`, `SameRuntimeVerification`).
        validate_graph_against(graph, self.config.profile_registry.as_deref())
            .map_err(graph_rejection)?;
        if rule == ValidationRule::Fresh
            && matches!(
                self.config.capacity.rotation,
                surge_core::capacity::RotationPolicy::Candidate { .. }
            )
        {
            return Err(AdmissionRejection::GraphInvalid(Box::new(
                EngineError::GraphInvalid(
                    "configured capacity rotation requires durable task ownership; start this graph through task Start".into(),
                ),
            )));
        }
        if rule == ValidationRule::Freeze
            && let Some(policy) = crate::engine::capacity_routes::freeze_policy(
                graph,
                self.config,
                self.agent_registry_for(config).as_deref(),
                self.storage.home(),
                working_dir.unwrap_or_else(|| self.storage.home()),
            )
            .map_err(|error| AdmissionRejection::QuotaPolicyInvalid(Box::new(error)))?
        {
            config.quota_recovery = policy;
        }
        validate_quota_policy(
            graph,
            &config.quota_recovery,
            self.config.profile_registry.as_deref(),
            self.agent_registry_for(config).as_deref(),
        )
        .map_err(|error| AdmissionRejection::QuotaPolicyInvalid(Box::new(error)))?;
        if rule != ValidationRule::Resume {
            // Fresh runs are untracked (no task context) and freezing seeds
            // the reservation's catalog: both seed it, exactly once.
            self.seed_profile_catalog(config).await?;
            validate_loop_seeds(graph, &config.seed_artifacts)
                .map_err(|error| AdmissionRejection::GraphInvalid(Box::new(error)))?;
        }
        Ok(())
    }

    fn refuse_reserved_task(&self, run_id: RunId, reason: &str) -> Result<(), AdmissionRejection> {
        if self
            .storage
            .work_items()
            .for_run(run_id)
            .map_err(storage)?
            .is_some()
        {
            return Err(AdmissionRejection::OwnershipRequired(reason.into()));
        }
        Ok(())
    }

    async fn refuse_active(&self, run_id: RunId) -> Result<(), AdmissionRejection> {
        if self.active_runs.read().await.contains_key(&run_id) {
            return Err(AdmissionRejection::AlreadyActive(run_id));
        }
        Ok(())
    }

    /// Fresh and task-claimed start.
    async fn start(
        &self,
        run_id: RunId,
        graph: Graph,
        worktree_path: PathBuf,
        mut run_config: EngineRunConfig,
        claim: Option<&WorkItemLaunchClaim>,
    ) -> Result<Admitted, AdmissionRejection> {
        let task_context = if let Some(claim) = claim {
            Some(
                crate::engine::work_items::pinned(self.storage, claim)
                    .map_err(engine)?
                    .2,
            )
        } else {
            self.refuse_reserved_task(run_id, "reserved task run requires host launch ownership")?;
            None
        };
        if let Some(inputs) = &run_config.owned_flow_inputs {
            let claim = claim.ok_or_else(|| {
                AdmissionRejection::OwnershipRequired(
                    "owned Flow inputs require real launch ownership".into(),
                )
            })?;
            inputs
                .validate_for(run_id, claim.binding(), &worktree_path)
                .map_err(|error| AdmissionRejection::OwnedFlowInputs(error.to_string()))?;
            self.storage
                .work_items()
                .validate_owned_flow_pending(claim)
                .map_err(|error| AdmissionRejection::OwnedFlowInputs(error.to_string()))?;
        }

        tracing::info!(
            target: "surge.path.exercised",
            path = "engine",
            kind = "start",
            run_id = %run_id,
            "entered engine path",
        );

        // A follow-up of a bootstrap run inherits exactly what the handoff
        // validated for the graph it was given (#98), through the seed path:
        // each inherited artifact is recorded once, and loop-seed validation
        // sees the inherited roadmap.
        if let Some(parent) = run_config.bootstrap_parent {
            self.inherit_bootstrap_handoff(parent, &graph, &mut run_config)
                .await?;
        }

        // A task-claimed start trusts its reservation: the same sequence
        // validated and froze this config at reservation time.
        if claim.is_none() {
            self.validate(&graph, &mut run_config, ValidationRule::Fresh, None)
                .await?;
        }
        run_config.memory_store_path =
            self.resolve_memory_store_path(run_config.memory_store_path.take());

        self.refuse_active(run_id).await?;
        if !worktree_path.exists() {
            return Err(AdmissionRejection::WorktreeMissing(worktree_path));
        }

        let writer = self.start_writer(run_id, &worktree_path, claim).await?;
        let artifact_store = ArtifactStore::new(self.storage.home().join("runs"));
        let prepared = self
            .append_startup(
                &writer,
                &artifact_store,
                run_id,
                &graph,
                &worktree_path,
                &run_config,
                claim,
                task_context,
            )
            .await;
        if let Err(rejection) = prepared {
            return Err(close_rejected(writer, rejection).await);
        }
        // Fresh start is "latest, right after the startup prefix" (#100): the
        // same reconstruction a resume of this log would make.
        let execution = match execution_state::load(&writer.reader(), Position::Latest)
            .await
            .map_err(state_rejection)
        {
            Ok(execution) => execution,
            // The startup prefix is durable: end the run visibly, as a failure
            // inside the run task would, so recovery never resumes a start
            // that never ran.
            Err(rejection) => return Err(fail_and_close(writer, rejection).await),
        };
        self.assemble(
            run_id,
            execution,
            worktree_path,
            run_config,
            writer,
            artifact_store,
            claim,
            None,
        )
        .await
    }

    /// Append a started run's startup events (and, for a task, its binding
    /// and owned-Flow startup acknowledgement).
    #[allow(clippy::too_many_arguments)]
    async fn append_startup(
        &self,
        writer: &RunWriter,
        artifact_store: &ArtifactStore,
        run_id: RunId,
        graph: &Graph,
        worktree_path: &Path,
        run_config: &EngineRunConfig,
        claim: Option<&WorkItemLaunchClaim>,
        task_context: Option<surge_core::work_item::WorkItemContext>,
    ) -> Result<(), AdmissionRejection> {
        let mut events =
            startup::build_startup_events(artifact_store, run_id, graph, worktree_path, run_config)
                .await
                .map_err(internal)?;
        if let Some(context) = task_context {
            if let Some(claim) = claim {
                self.storage
                    .work_items()
                    .validate_claim(claim)
                    .map_err(|error| AdmissionRejection::OwnershipRequired(error.to_string()))?;
            }
            startup::append_task_binding(artifact_store, run_id, context, &mut events)
                .await
                .map_err(internal)?;
        }
        writer.append_events(events).await.map_err(storage)?;
        if let Some(inputs) = &run_config.owned_flow_inputs {
            let claim = claim.ok_or_else(|| {
                AdmissionRejection::OwnershipRequired("owned Flow startup owner missing".into())
            })?;
            crate::engine::work_items::validate_resume(self.storage, claim)
                .await
                .map_err(engine)?;
            self.storage
                .work_items()
                .acknowledge_owned_flow_startup(claim, inputs.manifest())
                .map_err(|error| AdmissionRejection::OwnedFlowInputs(error.to_string()))?;
        }
        Ok(())
    }

    /// Reload the bootstrap handoff for `graph` and add its inherited
    /// artifacts to the run's seeds, refusing a caller seed that would record
    /// one of them a second time.
    async fn inherit_bootstrap_handoff(
        &self,
        parent: RunId,
        graph: &Graph,
        run_config: &mut EngineRunConfig,
    ) -> Result<(), AdmissionRejection> {
        use crate::bootstrap::handoff::{HandoffContext, HandoffRejection, reload};
        let expected = graph.identity().map_err(|error| {
            AdmissionRejection::Internal(Box::new(EngineError::Internal(error.to_string())))
        })?;
        let agents = self.agent_registry_for(run_config);
        let context = HandoffContext {
            profiles: self.config.profile_registry.as_deref(),
            agents: agents.as_deref(),
        };
        let handoff = reload(self.storage, parent, &context, expected)
            .await
            .map_err(AdmissionRejection::BootstrapHandoff)?;
        let seeds = handoff
            .seed_artifacts()
            .map_err(AdmissionRejection::BootstrapHandoff)?;
        if let Some(collision) = run_config
            .seed_artifacts
            .iter()
            .find(|seed| seeds.iter().any(|inherited| inherited.name == seed.name))
        {
            return Err(AdmissionRejection::BootstrapHandoff(
                HandoffRejection::Internal(format!(
                    "seed {} would duplicate the artifact inherited from bootstrap parent {parent}",
                    collision.name
                )),
            ));
        }
        run_config.seed_artifacts.extend(seeds);
        Ok(())
    }

    async fn start_writer(
        &self,
        run_id: RunId,
        worktree_path: &Path,
        claim: Option<&WorkItemLaunchClaim>,
    ) -> Result<RunWriter, AdmissionRejection> {
        if claim.is_some()
            && self
                .storage
                .get_run(&run_id)
                .await
                .map_err(storage)?
                .is_some()
        {
            let inspected = self.storage.inspect_run(run_id).await.map_err(storage)?;
            if !matches!(
                inspected.database,
                surge_persistence::runs::inspection::RunDatabaseInspection::Present { ref events }
                    if events.is_empty()
            ) {
                return Err(AdmissionRejection::NonEmptyJournal);
            }
            self.storage.open_run_writer(run_id).await.map_err(storage)
        } else {
            self.storage
                .create_run(run_id, worktree_path, None)
                .await
                .map_err(storage)
        }
    }

    /// Replayed and task-claimed resume.
    async fn resume(
        &self,
        run_id: RunId,
        worktree_path: PathBuf,
        claim: Option<&WorkItemLaunchClaim>,
        continue_control: Option<ContinueEvidence>,
        catalog: Option<Arc<surge_acp::Registry>>,
    ) -> Result<Admitted, AdmissionRejection> {
        tracing::info!(
            target: "surge.path.exercised",
            path = "engine",
            kind = "resume",
            run_id = %run_id,
            "entered engine path",
        );
        self.refuse_active(run_id).await?;

        // ADR-0021: stop prior MCP groups still led by their recorded process;
        // refuse only when a group cannot be stopped or ownership is unclear.
        // Before any registry change or effect admission.
        if let Err(diagnostic) =
            crate::engine::writer_coverage::stop_and_assess_mcp_cleanup(self.storage, run_id)
                .await
                .map_err(engine)?
        {
            tracing::warn!(%run_id, %diagnostic, "resume refused: prior MCP process group is not stopped");
            return Err(AdmissionRejection::McpGroupNotStopped(diagnostic));
        }

        // Read-only state first: refuse a conflicting terminal history (#95)
        // and validate the resumed graph before anything durable changes.
        let preview = self.load_state(run_id).await?;
        refuse_terminal_conflict(run_id, &preview.terminal_verdict)?;
        let run_config = if preview.terminal_verdict.terminal().is_none() {
            let mut run_config = self.restored_run_config(claim, &preview)?;
            if let Some(catalog) = catalog {
                run_config.agent_registry = Some(catalog);
            }
            self.validate(
                &preview.graph,
                &mut run_config,
                ValidationRule::Resume,
                None,
            )
            .await?;
            Some(run_config)
        } else {
            None
        };

        let (writer, was_parked) = self.open_resumed_writer(run_id).await?;

        let mut state = match self.load_state(run_id).await {
            Ok(state) => state,
            Err(rejection) => return Err(close_rejected(writer, rejection).await),
        };
        tracing::debug!(%run_id, position = state.position, "resume state loaded");
        // A conflict that appeared after the read-only check is still never
        // settled on one claim nor re-executed (#95).
        if let Err(rejection) = refuse_terminal_conflict(run_id, &state.terminal_verdict) {
            return Err(close_rejected(writer, rejection).await);
        }
        if let Some(claim) = state.terminal_verdict.terminal() {
            let outcome = RunOutcome::from_terminal(&claim.outcome);
            return Ok(Admitted::Terminal { writer, outcome });
        }
        // The log turned non-terminal between the two reads: impossible for
        // an append-only log, refused rather than guessed.
        let Some(run_config) = run_config else {
            return Err(close_rejected(
                writer,
                AdmissionRejection::Storage("terminal run log changed during resume".into()),
            )
            .await);
        };

        // A committed unanswered gate survives an explicit Suspend/Continue
        // with its original identity. After an unplanned owner loss, retire
        // only unanswered HumanGate occurrences so a stale card cannot
        // authorize the resumed run; normal stage execution emits a fresh ID.
        if claim.is_none() {
            state.memory.gate_decisions.retain(|_, decision| {
                decision.response.is_some()
                    || decision.suspended
                    || decision.purpose != surge_core::run_state::GateDecisionPurpose::HumanGate
            });
        }
        let artifact_store = ArtifactStore::new(self.storage.home().join("runs"));
        self.assemble(
            run_id,
            state,
            worktree_path,
            run_config,
            writer,
            artifact_store,
            claim,
            Some(ResumeEvidence {
                was_parked,
                continue_control,
            }),
        )
        .await
    }

    /// Clear a parked run's registry row (mandatory), open its writer, and
    /// record the wake (best-effort).
    async fn open_resumed_writer(
        &self,
        run_id: RunId,
    ) -> Result<(RunWriter, bool), AdmissionRejection> {
        // ADR-0016 M3: a parked run's registry row must not survive into the
        // resumed execution, or the wake scan would select this now-running
        // run, find no recorded worktree, and fail it. Mandatory: a failed
        // read or clear refuses the resume.
        let was_parked = matches!(
            self.storage.get_run(&run_id).await.map_err(storage)?,
            Some(summary) if summary.status == surge_core::RunStatus::Parked
        );
        if was_parked {
            self.storage
                .clear_parked(&run_id, surge_core::RunStatus::Running)
                .await
                .map_err(|error| {
                    AdmissionRejection::Storage(format!(
                        "resume refused: could not clear the Parked registry row for \
                         {run_id} ({error}); resuming would leave it due for the wake scan, \
                         which would then fail this run for an unrecorded worktree"
                    ))
                })?;
        }

        let writer = self
            .storage
            .open_run_writer(run_id)
            .await
            .map_err(storage)?;
        // ADR-0016 M4: the wake is recorded here, the one choke point every
        // real wake goes through. Best-effort like `RunParked`'s own append.
        if was_parked
            && let Err(error) = writer
                .append_event(VersionedEventPayload::new(EventPayload::RunWokeFromPark {}))
                .await
        {
            tracing::warn!(
                target: "engine::capacity",
                %run_id,
                %error,
                "failed to append RunWokeFromPark; the run resumed but its log will not show \
                 when it woke, and RunState::Pipeline.parked will not fold clear"
            );
        }
        Ok((writer, was_parked))
    }

    async fn load_state(&self, run_id: RunId) -> Result<ExecutionState, AdmissionRejection> {
        let reader = self
            .storage
            .open_run_reader(run_id)
            .await
            .map_err(storage)?;
        execution_state::load(&reader, Position::Latest)
            .await
            .map_err(state_rejection)
    }

    /// Reconstruct a resumed run's config: the reservation's frozen config
    /// for a task run (default otherwise), with the persisted MCP servers,
    /// frozen budget and edit-loop cap re-armed, so a daemon restart never
    /// loosens the limits set at start.
    fn restored_run_config(
        &self,
        claim: Option<&WorkItemLaunchClaim>,
        replayed: &ExecutionState,
    ) -> Result<EngineRunConfig, AdmissionRejection> {
        let mut config = match claim {
            Some(claim) => {
                crate::engine::work_items::pinned(self.storage, claim)
                    .map_err(engine)?
                    .3
            },
            None => EngineRunConfig::default(),
        };
        if let Some(persisted) = &replayed.run_config {
            if config.owned_flow_inputs.is_none() {
                config.mcp_servers.clone_from(&persisted.mcp_servers);
            }
            config.budget = persisted.budget;
            if let Some(cap) = persisted.bootstrap_edit_loop_cap {
                config.bootstrap.edit_loop_cap = cap;
            }
        }
        // `memory_store_path` is never persisted: a resumed run's only
        // source is the engine-level fallback, the same one start consults.
        config.memory_store_path = self.resolve_memory_store_path(config.memory_store_path.take());
        Ok(config)
    }

    /// Build the per-run pieces every admitted run carries. A rejection here
    /// closes the open writer.
    #[allow(clippy::too_many_arguments)]
    async fn assemble(
        &self,
        run_id: RunId,
        execution: ExecutionState,
        worktree_path: PathBuf,
        run_config: EngineRunConfig,
        writer: RunWriter,
        artifact_store: ArtifactStore,
        claim: Option<&WorkItemLaunchClaim>,
        resume_evidence: Option<ResumeEvidence>,
    ) -> Result<Admitted, AdmissionRejection> {
        let effect_fence = match self.owned_flow_effect_fence(&run_config, claim) {
            Ok(fence) => fence,
            Err(rejection) => return Err(close_rejected(writer, rejection).await),
        };
        let mcp_registry = self.resolve_run_mcp_registry(
            &run_config,
            &worktree_path,
            &writer,
            effect_fence.as_ref(),
        );
        let mcp_servers = mcp_registry
            .as_deref()
            .map_or_else(Vec::new, surge_mcp::McpRegistry::configured_servers);
        let (capacity_ledger, capacity_estimator) = self.capacity_ports_for(&writer);
        let agent_registry = self.agent_registry_for(&run_config);
        Ok(Admitted::Run(Box::new(AdmittedRun {
            run_id,
            execution,
            worktree_path,
            run_config,
            writer,
            artifact_store,
            claim: claim.cloned(),
            resume_evidence,
            effect_fence,
            mcp_registry,
            mcp_servers,
            agent_registry,
            capacity_ledger,
            capacity_estimator,
        })))
    }

    fn owned_flow_effect_fence(
        &self,
        config: &EngineRunConfig,
        claim: Option<&WorkItemLaunchClaim>,
    ) -> Result<Option<Arc<OwnedFlowEffectFence>>, AdmissionRejection> {
        if config.owned_flow_inputs.is_none() {
            return Ok(None);
        }
        let claim = claim.ok_or_else(|| {
            AdmissionRejection::OwnedFlowInputs(
                "owned Flow requires retained launch ownership".into(),
            )
        })?;
        self.storage
            .work_items()
            .validate_owned_flow_effect(claim)
            .map_err(|_| {
                AdmissionRejection::OwnedFlowInputs(
                    "owned Flow current control forbids dispatch".into(),
                )
            })?;
        Ok(Some(Arc::new(OwnedFlowEffectFence::new(
            self.storage.work_items(),
            claim.clone(),
        ))))
    }

    /// The per-run MCP registry: owned-Flow inputs, else the run's own server
    /// list, else the engine-wide registry's servers.
    fn resolve_run_mcp_registry(
        &self,
        run_config: &EngineRunConfig,
        worktree_path: &Path,
        writer: &RunWriter,
        effect_fence: Option<&Arc<OwnedFlowEffectFence>>,
    ) -> Option<Arc<surge_mcp::McpRegistry>> {
        let configs = if let Some(inputs) = &run_config.owned_flow_inputs {
            let servers = inputs.servers();
            if servers.is_empty() {
                return None;
            }
            servers.to_vec()
        } else if run_config.mcp_servers.is_empty() {
            self.mcp_registry
                .map(|registry| registry.configured_servers())?
        } else {
            run_config.mcp_servers.clone()
        };
        let observer: Arc<dyn surge_mcp::writer_observer::HostWriterObserver> =
            Arc::new(crate::engine::writer_coverage::McpWriterObserver {
                recorder: writer.event_recorder(),
                invocation: surge_core::id::StageInvocationId::new(),
                effect_fence: effect_fence.cloned(),
            });
        Some(Arc::new(
            surge_mcp::McpRegistry::from_config_owned_on_demand(
                &configs,
                Some(worktree_path),
                &observer,
            ),
        ))
    }

    /// The per-run memory-store override, else the engine-level fallback.
    fn resolve_memory_store_path(&self, per_run: Option<PathBuf>) -> Option<PathBuf> {
        per_run.or_else(|| self.config.memory_store_path.clone())
    }

    /// The engine-wide capacity ledger (an exhausted runtime is a fact about
    /// the runtime, not any one run) and this run's work estimator.
    fn capacity_ports_for(
        &self,
        writer: &RunWriter,
    ) -> (
        Arc<dyn crate::engine::capacity::CapacityLedger>,
        Arc<dyn crate::engine::capacity::WorkEstimator>,
    ) {
        let ledger: Arc<dyn crate::engine::capacity::CapacityLedger> = Arc::new(
            crate::engine::capacity::PersistentCapacityLedger::new(self.storage.clone()),
        );
        let history: Arc<dyn crate::engine::capacity::ArchetypeHistory> = Arc::new(
            crate::engine::capacity::PersistentArchetypeHistory::new(self.storage.clone()),
        );
        let estimator: Arc<dyn crate::engine::capacity::WorkEstimator> = Arc::new(
            crate::engine::capacity::RunHistoryWorkEstimator::new(writer.reader())
                .with_history(history),
        );
        (ledger, estimator)
    }

    /// Seed the resolved profile registry as the `profile_catalog` run
    /// artifact so bootstrap profiles can bind Agent nodes to profiles that
    /// actually exist. Runtimes that cannot start, or whose quota is
    /// exhausted right now, are marked unavailable. A no-op without a
    /// profile registry.
    async fn seed_profile_catalog(
        &self,
        run_config: &mut EngineRunConfig,
    ) -> Result<(), AdmissionRejection> {
        let Some(registry) = self.config.profile_registry.as_ref() else {
            return Ok(());
        };
        let run_agents = self.agent_registry_for(run_config);
        let agents = crate::engine::capacity::run_agent_registry_or_builtin(run_agents.as_deref());
        let mut unavailable = std::collections::BTreeMap::new();
        for runtime in crate::profile_loader::catalog_runtimes(registry, run_agents.as_deref()) {
            if let Some(entry) = agents.find_normalized(&runtime)
                && let Err(error) = surge_acp::agent_env::resolve(&entry.id, &entry.env)
            {
                unavailable.insert(runtime, format!("not configured: {error}"));
                continue;
            }
            if let Ok(status) = self.storage.capacity_status(&runtime).await
                && let Some(why) =
                    crate::engine::capacity::exhausted_reason(&status, chrono::Utc::now())
            {
                unavailable.insert(runtime, why);
            }
        }
        let catalog = crate::profile_loader::render_profile_catalog_with(
            registry,
            run_agents.as_deref(),
            &unavailable,
        );
        let seed = RunSeedArtifact::new(
            startup::PROFILE_CATALOG_ARTIFACT_NAME,
            startup::PROFILE_CATALOG_ARTIFACT_RELPATH,
            catalog,
            startup::PROFILE_CATALOG_PRODUCER_NODE,
        )
        .map_err(|error| {
            AdmissionRejection::Internal(Box::new(EngineError::Internal(format!(
                "profile catalog producer key: {error}"
            ))))
        })?;
        run_config.seed_artifacts.push(seed);
        Ok(())
    }
}

/// Map a helper's `EngineError` into a rejection, keeping storage faults
/// distinct.
fn engine(error: EngineError) -> AdmissionRejection {
    match error {
        EngineError::Storage(message) => AdmissionRejection::Storage(message),
        EngineError::WorkItemRejected(reason) => AdmissionRejection::OwnedFlowInputs(reason),
        other => AdmissionRejection::Internal(Box::new(other)),
    }
}

fn internal(error: EngineError) -> AdmissionRejection {
    match error {
        EngineError::Storage(message) => AdmissionRejection::Storage(message),
        other => AdmissionRejection::Internal(Box::new(other)),
    }
}

fn state_rejection(error: execution_state::ExecutionStateLoadError) -> AdmissionRejection {
    if error.is_storage() {
        AdmissionRejection::Storage(error.to_string())
    } else {
        AdmissionRejection::Internal(Box::new(EngineError::from(error)))
    }
}

/// Refuse a conflicting terminal history (#95).
fn refuse_terminal_conflict(
    run_id: RunId,
    verdict: &surge_core::run_verdict::TerminalVerdict,
) -> Result<(), AdmissionRejection> {
    if let surge_core::run_verdict::TerminalVerdict::Conflict { .. } = verdict {
        let seqs = verdict.conflicting_seqs();
        tracing::warn!(%run_id, ?seqs, "resume refused: conflicting terminal history");
        return Err(AdmissionRejection::TerminalConflict { run_id, seqs });
    }
    Ok(())
}

/// Close the writer opened for a rejected admission, keeping both failures
/// (the original rejection keeps its concrete variant as the source).
/// Record `RunFailed` for a run whose startup prefix is already durable, then
/// close its writer.
async fn fail_and_close(writer: RunWriter, rejection: AdmissionRejection) -> AdmissionRejection {
    if let Err(error) = writer
        .append_event(VersionedEventPayload::new(EventPayload::RunFailed {
            error: format!("run state could not be loaded after startup: {rejection}"),
        }))
        .await
    {
        tracing::error!(%error, "could not record RunFailed for a run whose startup state did not load");
    }
    close_rejected(writer, rejection).await
}

async fn close_rejected(writer: RunWriter, rejection: AdmissionRejection) -> AdmissionRejection {
    match writer.close().await {
        Ok(()) => rejection,
        Err(close) => {
            AdmissionRejection::Internal(Box::new(EngineError::OperationAndWriterClose {
                operation: Box::new(rejection.into()),
                close: Box::new(close),
            }))
        },
    }
}
