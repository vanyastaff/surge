//! Outcome acceptance: whether an agent's reported outcome is accepted, and
//! what is committed when it is (#94).
//!
//! The agent stage hands every candidate outcome to [`accept`] and acts on the
//! answer. The rules run in this order, and the first rejection wins:
//!
//! 1. the node's `on_outcome` hook chain;
//! 2. verification authority — a `Verified` outcome needs a read-only sandbox;
//! 3. verification binding and sealing (with the historical file-audit
//!    exemption; a `FailedVerification` report is sealed, never rejected);
//! 4. the resolved profile's artifact contracts.
//!
//! A rejection is recorded once (`OutcomeRejectedByHook`), counted against the
//! [`RejectionBudget`] and answered with feedback for the agent's next turn.
//! An accepted outcome captures its artifacts first (`ArtifactProduced`, the
//! sealed verification report), applies the Flow Generator's outcome rewrite,
//! then appends one batch: `OutcomeReported`, the task-ledger event,
//! discovered-task events, and the `StageOutcomeCommitted` marker whose effect
//! count and hash cover exactly the preceding batch entries.
//!
//! This module never writes `StageFailed`, never closes the session and never
//! decides that a stage has terminally failed: an exhausted budget is returned
//! as [`StageError::OutcomeBudgetExhausted`] for the run task to resolve.
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use surge_core::artifact_contract::{ArtifactDiagnosticSeverity, validate_artifact};
use surge_core::content_hash::ContentHash;
use surge_core::hooks::{Hook, HookTrigger};
use surge_core::keys::{NodeKey, OutcomeKey};
use surge_core::node::{LedgerEffect, OutcomeDecl};
use surge_core::profile::registry::ResolvedProfile;
use surge_core::roadmap::RoadmapTaskId;
use surge_core::run_event::{EventPayload, VersionedEventPayload};
use surge_core::{ArtifactKind, ProfileArtifactDeclaration};
use surge_persistence::runs::run_writer::RunWriter;

use super::StageError;
use super::agent::AgentStageParams;
use crate::engine::hooks::{HookContext, HookOutcome, record_hook_executed};

/// One outcome the agent reported, as received.
pub(crate) struct Candidate {
    pub outcome: OutcomeKey,
    pub summary: String,
    pub artifacts_produced: Vec<String>,
    pub verification_report: Option<Box<surge_core::roadmap::VerificationReportArtifact>>,
}

/// The stage facts acceptance needs for one provider attempt.
pub(crate) struct AcceptanceContext<'c, 's> {
    pub stage: &'c AgentStageParams<'s>,
    pub effective_hooks: &'c [Hook],
    pub sandbox_mode: surge_core::sandbox::SandboxMode,
    pub resolved_profile: Option<&'c ResolvedProfile>,
    pub verification_input: Option<&'c surge_core::verification_evidence::VerificationBinding>,
    pub verification_input_seq: surge_persistence::runs::EventSeq,
    pub stage_context: &'c surge_core::stage_tool::StageToolContext,
    pub provider_invocation: surge_core::id::StageInvocationId,
    pub session_id: surge_core::id::SessionId,
}

/// The answer for one candidate.
#[derive(Debug)]
pub(crate) enum Acceptance {
    /// The outcome and its whole commit batch are durable.
    Accepted(OutcomeKey),
    /// The rejection is durable; the agent gets `feedback` and may retry
    /// `remaining` more times.
    Rejected { feedback: String, remaining: u32 },
}

/// How many rejected outcomes a stage visit may still absorb, and which
/// outcome keys were rejected so far.
#[derive(Debug)]
pub(crate) struct RejectionBudget {
    attempts: u32,
    max_rejections: u32,
    rejected: Vec<OutcomeKey>,
}

impl RejectionBudget {
    /// A fresh budget allowing `max_rejections` rejections before exhaustion.
    #[must_use]
    pub(crate) fn new(max_rejections: u32) -> Self {
        Self {
            attempts: 0,
            max_rejections,
            rejected: Vec::new(),
        }
    }

    /// Rejections recorded so far in this stage visit.
    #[must_use]
    pub(crate) fn attempts(&self) -> u32 {
        self.attempts
    }

    /// Record one rejection durably and count it.
    ///
    /// # Errors
    /// `Storage` when the rejection cannot be recorded;
    /// `OutcomeBudgetExhausted` once rejections exceed the budget.
    async fn reject(
        &mut self,
        writer: &RunWriter,
        node: &NodeKey,
        rejection: Rejection,
    ) -> Result<Acceptance, StageError> {
        writer
            .append_event(VersionedEventPayload::new(
                EventPayload::OutcomeRejectedByHook {
                    node: node.clone(),
                    outcome: rejection.outcome.clone(),
                    hook_id: rejection.hook_id.clone(),
                },
            ))
            .await
            .map_err(|e| StageError::Storage(e.to_string()))?;
        self.attempts += 1;
        if !self.rejected.contains(&rejection.outcome) {
            self.rejected.push(rejection.outcome.clone());
        }
        tracing::info!(
            target: "engine::stage::agent",
            node = %node,
            outcome = %rejection.outcome,
            hook_id = %rejection.hook_id,
            attempt = self.attempts,
            max = self.max_rejections,
            reason = %rejection.reason,
            source = %rejection.rule,
            "outcome rejected; awaiting agent retry"
        );
        if self.attempts > self.max_rejections {
            return Err(StageError::OutcomeBudgetExhausted {
                hook_id: rejection.hook_id,
                rule: rejection.rule.to_owned(),
                rejected: std::mem::take(&mut self.rejected),
            });
        }
        Ok(Acceptance::Rejected {
            feedback: validation_retry_prompt(&rejection.feedback),
            remaining: self.max_rejections + 1 - self.attempts,
        })
    }
}

/// One rule's rejection of a candidate.
struct Rejection {
    outcome: OutcomeKey,
    hook_id: String,
    reason: String,
    rule: &'static str,
    /// What the agent is told, before the shared retry instructions.
    feedback: String,
}

/// Decide one candidate outcome: accept it and commit its batch, or record
/// its rejection. See the module doc for rule order and batch shape.
///
/// # Errors
/// `OutcomeBudgetExhausted` when this rejection exceeds the budget;
/// `Storage` for log, artifact-store or worktree I/O failures; `Internal`
/// for a duplicate logical artifact name or an unbuildable commit;
/// `EditLoopCapExceeded` / `Internal` from the Flow Generator rewrite.
#[must_use = "an accepted outcome ends the stage; a rejection needs a retry turn"]
pub(crate) async fn accept(
    ctx: &AcceptanceContext<'_, '_>,
    candidate: Candidate,
    budget: &mut RejectionBudget,
) -> Result<Acceptance, StageError> {
    let Candidate {
        outcome,
        summary,
        artifacts_produced,
        verification_report,
    } = candidate;
    match check_rules(ctx, outcome, verification_report, &artifacts_produced).await? {
        Verdict::Reject(rejection) => {
            budget
                .reject(ctx.stage.writer, ctx.stage.node, rejection)
                .await
        },
        Verdict::Pass {
            outcome,
            accepted_report,
        } => commit_accepted(ctx, outcome, summary, &artifacts_produced, accepted_report)
            .await
            .map(Acceptance::Accepted),
    }
}

/// The rules' answer for one candidate.
enum Verdict {
    Reject(Rejection),
    Pass {
        outcome: OutcomeKey,
        /// The sealed verification report, when one is committed.
        accepted_report: Option<Box<surge_core::roadmap::VerificationReportArtifact>>,
    },
}

/// Run the rejection rules in order; the first rejection wins.
async fn check_rules(
    ctx: &AcceptanceContext<'_, '_>,
    outcome: OutcomeKey,
    verification_report: Option<Box<surge_core::roadmap::VerificationReportArtifact>>,
    artifacts_produced: &[String],
) -> Result<Verdict, StageError> {
    let p = ctx.stage;

    // 1. on_outcome hook chain, before anything about the outcome is persisted.
    let hook_ctx = HookContext::for_node(p.node)
        .with_writer(p.writer, ctx.provider_invocation)
        .with_worktree_path(p.worktree_path)
        .with_session(ctx.session_id)
        .with_outcome(&outcome);
    let outcome_chain = p
        .hook_executor
        .run_hooks(ctx.effective_hooks, HookTrigger::OnOutcome, &hook_ctx)
        .await;
    for record in outcome_chain.executed() {
        record_hook_executed(p.writer, record).await;
    }
    if let HookOutcome::Reject {
        reason, hook_id, ..
    } = &outcome_chain
    {
        return Ok(Verdict::Reject(Rejection {
            feedback: format!("{hook_id}: {reason}"),
            outcome,
            hook_id: hook_id.clone(),
            reason: reason.clone(),
            rule: "on_outcome hook",
        }));
    }

    let effect = outcome_ledger_effect(p.declared_outcomes, &outcome);

    // 2. Sealed-verifier gate. A `Verified` outcome is the sole path to a
    // verified `Completed` ledger status, so it may only come from a sealed
    // (read-only, no-network, no-shell) sandbox.
    if effect == LedgerEffect::Verified
        && ctx.sandbox_mode != surge_core::sandbox::SandboxMode::ReadOnly
    {
        return Ok(Verdict::Reject(Rejection {
            outcome,
            hook_id: "verification_authority".into(),
            reason: "a Verified outcome requires a sealed read-only sandbox (no workspace writes)"
                .into(),
            rule: "verification authority",
            feedback: "A verified outcome requires a sealed read-only sandbox. Report an \
                       appropriate non-verified outcome instead."
                .into(),
        }));
    }

    // 3. Verification binding and sealing.
    let accepted_report =
        match verification_rule(ctx, &outcome, effect, verification_report).await? {
            Ok(report) => report,
            Err(reason) => {
                return Ok(Verdict::Reject(Rejection {
                    outcome,
                    hook_id: "verification_binding".into(),
                    feedback: reason.clone(),
                    reason,
                    rule: "verification binding",
                }));
            },
        };

    // 4. Profile artifact contracts.
    if let Some(rejection) = validate_profile_artifact_contracts(
        ctx.resolved_profile,
        p.agent_config.profile.as_str(),
        &outcome,
        artifacts_produced,
        p.worktree_path,
        accepted_report.is_some(),
    )
    .await?
    {
        return Ok(Verdict::Reject(Rejection {
            feedback: format!("{}: {}", rejection.hook_id, rejection.reason),
            outcome,
            hook_id: rejection.hook_id,
            reason: rejection.reason,
            rule: "profile artifact contract",
        }));
    }
    Ok(Verdict::Pass {
        outcome,
        accepted_report,
    })
}

/// The verification rule: a `Verified` outcome must bind and seal its report
/// (unless the historical file-audit contract validates the file instead); a
/// `FailedVerification` report is sealed, never rejected. `Ok(Err(reason))`
/// is a rejection.
async fn verification_rule(
    ctx: &AcceptanceContext<'_, '_>,
    outcome: &OutcomeKey,
    effect: LedgerEffect,
    verification_report: Option<Box<surge_core::roadmap::VerificationReportArtifact>>,
) -> Result<Result<Option<Box<surge_core::roadmap::VerificationReportArtifact>>, String>, StageError>
{
    let p = ctx.stage;
    let historical_file_audit = effect == LedgerEffect::Verified
        && p.active_task_id.is_none()
        && verification_report.is_none()
        && ctx.resolved_profile.is_some_and(|profile| {
            profile.profile.outcomes.iter().any(|declaration| {
                &declaration.id == outcome
                    && declaration
                        .produced_artifacts
                        .iter()
                        .any(|artifact| artifact.contract.kind == ArtifactKind::VerificationReport)
            })
        });
    if historical_file_audit {
        return Ok(Ok(None));
    }
    match effect {
        LedgerEffect::Verified => Ok(super::verification::seal(
            p,
            ctx.verification_input,
            verification_report.map(|report| *report),
            ctx.verification_input_seq,
        )
        .await
        .map(|report| Some(Box::new(report)))),
        LedgerEffect::FailedVerification => Ok(Ok(verification_report
            .map(|report| super::verification::seal_failure(p, ctx.verification_input, *report))
            .transpose()
            .map_err(StageError::Internal)?
            .map(Box::new))),
        _ => Ok(Ok(None)),
    }
}

/// Commit an accepted outcome: artifacts first, then the Flow Generator
/// rewrite, then the batch and its commit marker.
async fn commit_accepted(
    ctx: &AcceptanceContext<'_, '_>,
    mut outcome: OutcomeKey,
    mut summary: String,
    artifacts_produced: &[String],
    accepted_report: Option<Box<surge_core::roadmap::VerificationReportArtifact>>,
) -> Result<OutcomeKey, StageError> {
    let p = ctx.stage;
    let captured = capture_artifacts(p, artifacts_produced).await?;
    let mut produced_hashes = captured.hashes;
    if let Some(report) = &accepted_report {
        let bytes = toml::to_string(report)
            .map_err(|error| StageError::Storage(error.to_string()))?
            .into_bytes();
        let artifact = p
            .artifact_store
            .put(p.run_id, "verification-report", &bytes)
            .await
            .map_err(|error| StageError::Storage(error.to_string()))?;
        produced_hashes.insert("verification-report".into(), artifact.hash);
        p.writer
            .append_event(VersionedEventPayload::new(EventPayload::ArtifactProduced {
                node: p.node.clone(),
                artifact: artifact.hash,
                path: artifact.path,
                name: "verification-report".into(),
                source_path: None,
            }))
            .await
            .map_err(|error| StageError::Storage(error.to_string()))?;
    }
    if super::verification::verification_in_play(
        super::verification::VerificationPoint::Accepted {
            input: ctx.verification_input,
            active_task: p.active_task_id.is_some(),
        },
        p.run_memory,
    ) {
        super::verification::observe_after(
            p.writer,
            p.worktree_path,
            ctx.verification_input.map(|input| &input.subject).or(p
                .run_memory
                .verification
                .subject
                .as_ref()),
        )
        .await?;
    }
    if let Some((rewritten, rewritten_summary)) = flow_generator_rewrite(p).await? {
        outcome = rewritten;
        summary = rewritten_summary;
    }

    let mut committed = vec![VersionedEventPayload::new(EventPayload::OutcomeReported {
        node: p.node.clone(),
        outcome: outcome.clone(),
        summary,
    })];
    if let Some(task_id) = p.active_task_id.as_ref() {
        // The sealed-verifier gate guarantees a `Verified` effect only reaches
        // here from a read-only sandbox. The rewrite may change the outcome,
        // so its effect is read for the final key.
        if let Some(payload) = ledger_event(
            p.node,
            p.run_memory,
            task_id,
            outcome_ledger_effect(p.declared_outcomes, &outcome),
            &produced_hashes,
            accepted_report.map(|report| *report),
        )? {
            committed.push(VersionedEventPayload::new(payload));
        }
        // A malformed discovered-tasks artifact is logged and skipped; it
        // never fails the stage.
        if let Some(bytes) = captured.discovered_tasks.as_deref() {
            committed.extend(
                discovered_task_events(p.node, task_id, bytes)
                    .into_iter()
                    .map(VersionedEventPayload::new),
            );
        }
    }
    let effects_hash = ContentHash::compute(
        &serde_json::to_vec(&committed).map_err(|error| StageError::Storage(error.to_string()))?,
    );
    let effects_count = u32::try_from(committed.len())
        .map_err(|_| StageError::Internal("stage effects batch is too large".into()))?;
    let commit = surge_core::execution_recovery::commit::StageOutcomeCommit::new(
        ctx.stage_context.clone(),
        ctx.session_id,
        ctx.provider_invocation,
        outcome.clone(),
        effects_count,
        effects_hash,
    )
    .map_err(|error| StageError::Internal(error.to_string()))?;
    committed.push(VersionedEventPayload::new(
        EventPayload::StageOutcomeCommitted { commit },
    ));
    p.writer
        .append_events(committed)
        .await
        .map_err(|error| StageError::Storage(error.to_string()))?;
    Ok(outcome)
}

/// The Flow Generator's post-acceptance step: a failed validation rewrites
/// the outcome to its validation-failed key (so retries route through the
/// graph rather than this budget); an exceeded edit cap or a missing flow
/// ends the stage.
async fn flow_generator_rewrite(
    p: &AgentStageParams<'_>,
) -> Result<Option<(OutcomeKey, String)>, StageError> {
    if !crate::engine::bootstrap::is_flow_generator_profile(p.agent_config.profile.as_str()) {
        return Ok(None);
    }
    match crate::engine::bootstrap::run_flow_generator_post_processing_with_registry(
        p.node,
        p.run_memory,
        p.run_memory.bootstrap_edit_loop_cap.unwrap_or(0),
        p.worktree_path,
        p.writer,
        p.profile_registry.as_deref(),
    )
    .await?
    {
        crate::engine::bootstrap::FlowValidationDecision::Materialized => Ok(None),
        crate::engine::bootstrap::FlowValidationDecision::EditRequested { feedback } => {
            let outcome = OutcomeKey::try_from(crate::engine::bootstrap::VALIDATION_FAILED_OUTCOME)
                .map_err(|error| {
                    StageError::Internal(format!("validation retry outcome key: {error}"))
                })?;
            Ok(Some((
                outcome,
                format!("Flow Generator validation retry: {feedback}"),
            )))
        },
        crate::engine::bootstrap::FlowValidationDecision::CapExceeded { cap } => {
            Err(StageError::EditLoopCapExceeded {
                stage: surge_core::run_event::BootstrapStage::Flow,
                cap,
            })
        },
        crate::engine::bootstrap::FlowValidationDecision::MissingArtifact => {
            Err(StageError::Internal(
                "Flow Generator stage finished without producing flow.toml".into(),
            ))
        },
    }
}

/// Artifacts an accepted outcome captured into the store.
struct CapturedArtifacts {
    hashes: BTreeMap<String, ContentHash>,
    discovered_tasks: Option<Vec<u8>>,
}

/// Store every declared artifact and record one `ArtifactProduced` per path,
/// before the batch, so the fold populates `RunMemory.artifacts`
/// deterministically. A path that escapes the worktree, is missing or is
/// unreadable is logged and skipped; it does not fail the stage.
async fn capture_artifacts(
    p: &AgentStageParams<'_>,
    artifacts_produced: &[String],
) -> Result<CapturedArtifacts, StageError> {
    let mut produced_hashes: BTreeMap<String, ContentHash> = BTreeMap::new();
    let mut discovered_tasks_bytes: Option<Vec<u8>> = None;
    if artifacts_produced.is_empty() {
        return Ok(CapturedArtifacts {
            hashes: produced_hashes,
            discovered_tasks: None,
        });
    }
    let canonical_worktree = tokio::fs::canonicalize(p.worktree_path)
        .await
        .map_err(|e| StageError::Storage(e.to_string()))?;
    let stem_counts = artifact_stem_counts(artifacts_produced);
    let mut emitted_names = BTreeSet::new();
    for declared_path in artifacts_produced {
        let Some(relative_path) = safe_declared_artifact_path(declared_path) else {
            tracing::warn!(target: "engine::stage::agent", node = %p.node, path = %declared_path,
                "artifact path escapes worktree — skipping");
            continue;
        };
        let absolute = p.worktree_path.join(&relative_path);
        let canonical_path = match tokio::fs::canonicalize(&absolute).await {
            Ok(path) => path,
            Err(e) => {
                tracing::warn!(target: "engine::stage::agent", node = %p.node,
                    path = %declared_path, err = %e, "artifact path missing — skipping");
                continue;
            },
        };
        if !canonical_path.starts_with(&canonical_worktree) {
            tracing::warn!(target: "engine::stage::agent", node = %p.node,
                path = %declared_path, canonical_path = %canonical_path.display(),
                "artifact path escapes worktree — skipping");
            continue;
        }
        let mut bytes = match tokio::fs::read(&canonical_path).await {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!(target: "engine::stage::agent", node = %p.node,
                    path = %declared_path, err = %e, "artifact path missing — skipping");
                continue;
            },
        };
        // Project memory: stamp provenance (run + node) into an agent-authored
        // `.surge/memory/` note BEFORE it is content-addressed and stored, so
        // the store blob, the recorded hash and the worktree file all agree.
        if is_project_memory_note(&relative_path)
            && let Some(stamped) = stamp_memory_bytes(&bytes, p.run_id, p.node)
        {
            tokio::fs::write(&canonical_path, &stamped)
                .await
                .map_err(|e| StageError::Storage(e.to_string()))?;
            bytes = stamped;
        }
        let name = logical_artifact_name(&relative_path, declared_path, &stem_counts);
        if !emitted_names.insert(name.clone()) {
            return Err(StageError::Internal(format!(
                "duplicate artifact logical name '{name}' from declared path '{declared_path}'"
            )));
        }
        let artifact_ref = p
            .artifact_store
            .put(p.run_id, &name, &bytes)
            .await
            .map_err(|e| StageError::Storage(e.to_string()))?;
        produced_hashes.insert(name.clone(), artifact_ref.hash);
        if name == "discovered-tasks" {
            discovered_tasks_bytes = Some(bytes.clone());
        }
        tracing::info!(target: "engine::stage::agent", node = %p.node, name = %name,
            hash = %artifact_ref.hash, store_path = %artifact_ref.path.display(),
            "artifact_produced");
        p.writer
            .append_event(VersionedEventPayload::new(EventPayload::ArtifactProduced {
                node: p.node.clone(),
                artifact: artifact_ref.hash,
                path: artifact_ref.path,
                name,
                source_path: Some(relative_path),
            }))
            .await
            .map_err(|e| StageError::Storage(e.to_string()))?;
    }
    Ok(CapturedArtifacts {
        hashes: produced_hashes,
        discovered_tasks: discovered_tasks_bytes,
    })
}

/// Look up the [`LedgerEffect`] declared for `outcome` on this node, defaulting
/// to [`LedgerEffect::None`] when the outcome is not found (or declares none).
fn outcome_ledger_effect(declared: &[OutcomeDecl], outcome: &OutcomeKey) -> LedgerEffect {
    declared
        .iter()
        .find(|decl| &decl.id == outcome)
        .map_or(LedgerEffect::None, |decl| decl.ledger_effect)
}

/// Append the task-ledger event implied by `effect` for `task_id`.
///
/// - `ReadyForVerification` / `FailedVerification` → `TaskStatusChanged` (the
///   `from` status is read from the folded ledger, defaulting to `Pending`).
/// - `Verified` → `TaskVerified` with `evidence` = the produced
///   host-sealed `verification-report` hash and its exact bound report.
///   Missing reports are rejected; other artifacts cannot substitute as proof.
/// - `None` → no event.
fn ledger_event(
    node: &NodeKey,
    memory: &surge_core::run_state::RunMemory,
    task_id: &RoadmapTaskId,
    effect: LedgerEffect,
    produced_hashes: &BTreeMap<String, ContentHash>,
    sealed_report: Option<surge_core::roadmap::VerificationReportArtifact>,
) -> Result<Option<EventPayload>, StageError> {
    use surge_core::roadmap::RoadmapStatus;

    let payload = match effect {
        LedgerEffect::None => return Ok(None),
        LedgerEffect::ReadyForVerification | LedgerEffect::FailedVerification => {
            let to = if matches!(effect, LedgerEffect::ReadyForVerification) {
                RoadmapStatus::ReadyForVerification
            } else {
                RoadmapStatus::FailedVerification
            };
            let from = memory
                .ledger
                .tasks
                .get(task_id)
                .map_or(RoadmapStatus::Pending, |task| task.status);
            EventPayload::TaskStatusChanged {
                task_id: task_id.clone(),
                from,
                to,
                authority_node: node.clone(),
            }
        },
        LedgerEffect::Verified => {
            let evidence = produced_hashes
                .get("verification-report")
                .copied()
                .ok_or_else(|| StageError::Internal("missing sealed verification report".into()))?;
            EventPayload::TaskVerified {
                task_id: task_id.clone(),
                node: node.clone(),
                evidence,

                report: sealed_report,
            }
        },
    };
    Ok(Some(payload))
}

/// True when `relative_path` is an agent-authored project-memory note
/// (`.surge/memory/*.md`, excluding the human-facing `MEMORY.md` index).
fn is_project_memory_note(relative_path: &Path) -> bool {
    relative_path.starts_with(".surge/memory")
        && relative_path.extension().and_then(|ext| ext.to_str()) == Some("md")
        && relative_path.file_name().and_then(|name| name.to_str()) != Some("MEMORY.md")
}

/// Return `bytes` with a provenance comment prepended, or `None` when it is
/// already stamped (idempotent across retry/replay) or not UTF-8. Pure: the
/// caller writes the result to disk before content-addressing it. The marker is
/// an HTML comment, invisible in rendered markdown but visible in source and to
/// the next run's memory seed.
fn stamp_memory_bytes(
    bytes: &[u8],
    run_id: surge_core::id::RunId,
    node: &NodeKey,
) -> Option<Vec<u8>> {
    const MARKER: &str = "<!-- surge:memory";
    let text = std::str::from_utf8(bytes).ok()?;
    if text.trim_start().starts_with(MARKER) {
        return None;
    }
    Some(format!("{MARKER} run={run_id} node={} -->\n{text}", node.as_str()).into_bytes())
}

/// Parse a produced `discovered-tasks` artifact and append one
/// `TaskDiscovered` event per entry, attaching each to `discovered_from`.
///
/// Lenient: a malformed or invalid artifact is logged and skipped rather than
/// failing the stage — discovered work is advisory, and an AFK run should not
/// abort because a side artifact was ill-formed.
fn discovered_task_events(
    node: &NodeKey,
    discovered_from: &RoadmapTaskId,
    bytes: &[u8],
) -> Vec<EventPayload> {
    let text = match std::str::from_utf8(bytes) {
        Ok(text) => text,
        Err(error) => {
            tracing::warn!(
                target: "engine::stage::agent",
                node = %node,
                err = %error,
                "discovered-tasks artifact is not UTF-8 — skipping"
            );
            return Vec::new();
        },
    };
    let artifact: surge_core::DiscoveredTasksArtifact = match toml::from_str(text) {
        Ok(artifact) => artifact,
        Err(error) => {
            tracing::warn!(
                target: "engine::stage::agent",
                node = %node,
                err = %error,
                "discovered-tasks artifact failed to parse — skipping"
            );
            return Vec::new();
        },
    };
    let issues = artifact.validate();
    if !issues.is_empty() {
        tracing::warn!(
            target: "engine::stage::agent",
            node = %node,
            issues = ?issues,
            "discovered-tasks artifact is invalid — skipping"
        );
        return Vec::new();
    }
    artifact
        .tasks
        .into_iter()
        .map(|entry| EventPayload::TaskDiscovered {
            task_id: entry.id,
            discovered_from: discovered_from.clone(),
            title: entry.title,
        })
        .collect()
}

/// Follow-up turn after validation rejected a reported outcome.
fn validation_retry_prompt(feedback: &str) -> String {
    format!(
        "Your stage outcome was rejected by validation:\n{feedback}\n\nCorrect the artifacts or outcome, then call report_stage_outcome again with a new unique call_id. The stage is not complete until validation accepts the result."
    )
}

#[derive(Debug, Clone)]
pub(super) struct ArtifactContractRejection {
    hook_id: String,
    reason: String,
}

pub(super) async fn validate_profile_artifact_contracts(
    resolved_profile: Option<&ResolvedProfile>,
    profile_ref: &str,
    outcome: &OutcomeKey,
    artifacts_produced: &[String],
    worktree_path: &Path,
    inline_verification: bool,
) -> Result<Option<ArtifactContractRejection>, StageError> {
    let Some(profile) = resolved_profile else {
        return Ok(None);
    };
    let Some(profile_outcome) = profile
        .profile
        .outcomes
        .iter()
        .find(|candidate| candidate.id == *outcome)
    else {
        return Ok(None);
    };
    if profile_outcome.produced_artifacts.is_empty() {
        return Ok(None);
    }

    let canonical_worktree = tokio::fs::canonicalize(worktree_path)
        .await
        .map_err(|e| StageError::Storage(e.to_string()))?;
    let produced_paths: Vec<PathBuf> = artifacts_produced
        .iter()
        .filter_map(|declared_path| safe_declared_artifact_path(declared_path))
        .collect();

    for declaration in &profile_outcome.produced_artifacts {
        if inline_verification && declaration.contract.kind == ArtifactKind::VerificationReport {
            continue;
        }
        let matching_paths: Vec<&PathBuf> = produced_paths
            .iter()
            .filter(|path| artifact_declaration_matches_path(declaration, path))
            .collect();
        if matching_paths.is_empty() {
            return Ok(Some(artifact_contract_rejection(
                declaration.contract.kind,
                missing_declared_artifact_reason(outcome, declaration),
            )));
        }

        for relative_path in matching_paths {
            let Some(validation_input) =
                read_produced_artifact(worktree_path, &canonical_worktree, relative_path).await?
            else {
                return Ok(Some(artifact_contract_rejection(
                    declaration.contract.kind,
                    format!(
                        "artifact '{}' was reported but could not be read from the worktree",
                        normalize_artifact_path(relative_path)
                    ),
                )));
            };
            let content = String::from_utf8_lossy(&validation_input.bytes);
            if declaration.contract.kind == ArtifactKind::VerificationReport
                && toml::from_str::<surge_core::roadmap::VerificationReportArtifact>(&content)
                    .is_ok_and(|report| report.binding.is_some())
            {
                return Ok(Some(artifact_contract_rejection(
                    ArtifactKind::VerificationReport,
                    "worktree reports cannot supply host binding".into(),
                )));
            }

            // Flow-generator output is validated by the bootstrap post-processor,
            // which persists diagnostics and routes the bounded edit loop. Keep
            // path/readability checks here, but do not consume its retry there.
            if declaration.contract.kind == ArtifactKind::Flow
                && crate::engine::bootstrap::is_flow_generator_profile(profile_ref)
            {
                continue;
            }
            let report = validate_artifact(
                declaration.contract.kind,
                Some(validation_input.relative_path.as_path()),
                &content,
            );
            if report.is_valid() {
                continue;
            }
            return Ok(Some(artifact_contract_rejection(
                declaration.contract.kind,
                format_artifact_validation_reason(
                    declaration.contract.kind,
                    validation_input.relative_path.as_path(),
                    &report.diagnostics,
                ),
            )));
        }
    }

    Ok(None)
}

struct ProducedArtifactInput {
    relative_path: PathBuf,
    bytes: Vec<u8>,
}

async fn read_produced_artifact(
    worktree_path: &Path,
    canonical_worktree: &Path,
    relative_path: &Path,
) -> Result<Option<ProducedArtifactInput>, StageError> {
    let absolute = worktree_path.join(relative_path);
    let canonical_path = match tokio::fs::canonicalize(&absolute).await {
        Ok(path) => path,
        Err(error) => {
            tracing::warn!(
                target: "engine::stage::agent",
                relative_path = %normalize_artifact_path(relative_path),
                absolute_path = %absolute.display(),
                err = %error,
                "reported artifact could not be canonicalized"
            );
            return Ok(None);
        },
    };
    if !canonical_path.starts_with(canonical_worktree) {
        tracing::warn!(
            target: "engine::stage::agent",
            relative_path = %normalize_artifact_path(relative_path),
            canonical_path = %canonical_path.display(),
            canonical_worktree = %canonical_worktree.display(),
            "reported artifact canonical path escaped the worktree"
        );
        return Ok(None);
    }
    let bytes = match tokio::fs::read(&canonical_path).await {
        Ok(bytes) => bytes,
        Err(error) => {
            tracing::warn!(
                target: "engine::stage::agent",
                relative_path = %normalize_artifact_path(relative_path),
                canonical_path = %canonical_path.display(),
                err = %error,
                "reported artifact could not be read"
            );
            return Ok(None);
        },
    };
    Ok(Some(ProducedArtifactInput {
        relative_path: relative_path.to_path_buf(),
        bytes,
    }))
}

fn missing_declared_artifact_reason(
    outcome: &OutcomeKey,
    declaration: &ProfileArtifactDeclaration,
) -> String {
    let kind = declaration.contract.kind;
    match kind {
        ArtifactKind::Adr | ArtifactKind::Story => {
            let contract = kind.contract();
            format!(
                "outcome '{outcome}' must produce a {kind} artifact matching declared path '{}' (contract pattern '{}')",
                declaration.path, contract.canonical_path
            )
        },
        _ => format!(
            "outcome '{outcome}' must produce artifact '{}'",
            declaration.path
        ),
    }
}

fn artifact_contract_rejection(kind: ArtifactKind, reason: String) -> ArtifactContractRejection {
    ArtifactContractRejection {
        hook_id: format!("profile-artifact-contract:{kind}"),
        reason,
    }
}

fn format_artifact_validation_reason(
    kind: ArtifactKind,
    path: &Path,
    diagnostics: &[surge_core::ArtifactValidationDiagnostic],
) -> String {
    let errors = diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.severity == ArtifactDiagnosticSeverity::Error)
        .take(3)
        .map(|diagnostic| {
            let location = diagnostic
                .location
                .as_ref()
                .map(|location| format!(" at {location}"))
                .unwrap_or_default();
            format!("{}{}: {}", diagnostic.code, location, diagnostic.message)
        })
        .collect::<Vec<_>>()
        .join("; ");
    format!(
        "{kind} artifact '{}' failed contract validation: {errors}",
        normalize_artifact_path(path)
    )
}

fn artifact_declaration_matches_path(
    declaration: &ProfileArtifactDeclaration,
    relative_path: &Path,
) -> bool {
    let declared = normalize_profile_declared_path(&declaration.path);
    let actual = normalize_artifact_path(relative_path);
    if declared == actual {
        return true;
    }

    match declaration.contract.kind {
        ArtifactKind::Adr | ArtifactKind::Story => {
            declared == declaration.contract.kind.contract().canonical_path
                && declaration
                    .contract
                    .kind
                    .contract()
                    .accepts_path(relative_path)
        },
        _ => false,
    }
}

fn artifact_stem_counts(paths: &[String]) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for declared_path in paths {
        let Some(relative_path) = safe_declared_artifact_path(declared_path) else {
            continue;
        };
        let Some(stem) = artifact_stem(&relative_path) else {
            continue;
        };
        *counts.entry(stem).or_insert(0) += 1;
    }
    counts
}

fn logical_artifact_name(
    relative_path: &Path,
    declared_path: &str,
    stem_counts: &BTreeMap<String, usize>,
) -> String {
    let Some(stem) = artifact_stem(relative_path) else {
        return sanitize_artifact_name(declared_path);
    };
    if stem_counts.get(&stem).copied().unwrap_or_default() <= 1 {
        return stem;
    }
    path_based_artifact_name(relative_path).unwrap_or(stem)
}

fn artifact_stem(path: &Path) -> Option<String> {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.is_empty())
        .map(ToOwned::to_owned)
}

fn path_based_artifact_name(path: &Path) -> Option<String> {
    let normalized = normalize_artifact_path(path);
    let name = sanitize_artifact_name(&normalized);
    (!name.is_empty()).then_some(name)
}

fn sanitize_artifact_name(input: &str) -> String {
    let mut name = String::with_capacity(input.len());
    let mut last_was_separator = false;
    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' {
            name.push(ch);
            last_was_separator = false;
        } else if !last_was_separator {
            name.push('_');
            last_was_separator = true;
        }
    }
    name.trim_matches('_').to_string()
}

fn normalize_profile_declared_path(path: &str) -> String {
    safe_declared_artifact_path(path).map_or_else(
        || path.replace('\\', "/"),
        |path| normalize_artifact_path(&path),
    )
}

fn normalize_artifact_path(path: &Path) -> String {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part.to_string_lossy()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn safe_declared_artifact_path(declared_path: &str) -> Option<PathBuf> {
    if declared_path.trim().is_empty() {
        return None;
    }

    let path = Path::new(declared_path);
    let mut has_normal_component = false;
    for component in path.components() {
        match component {
            Component::Normal(_) => has_normal_component = true,
            Component::CurDir => {},
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }

    has_normal_component.then(|| path.to_path_buf())
}
