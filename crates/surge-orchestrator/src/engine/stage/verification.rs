//! Host-owned verifier input and inline report sealing.
use super::{StageError, agent::AgentStageParams};
use surge_core::{
    ContentHash,
    roadmap::{
        RoadmapArtifact, RoadmapTask, VerificationReportArtifact, VerificationReportOutcome,
    },
    run_event::{EventPayload, VersionedEventPayload},
    verification_evidence::{VerificationBinding, VerificationCriteria, VerificationSubject},
};

pub(super) async fn begin(
    p: &AgentStageParams<'_>,
) -> Result<Option<VerificationBinding>, StageError> {
    let verifying = p
        .declared_outcomes
        .iter()
        .any(|outcome| outcome.ledger_effect == surge_core::node::LedgerEffect::Verified);
    if !verifying && p.active_task_id.is_none() && p.run_memory.verification.subject.is_none() {
        return Ok(None);
    }
    let subject = if verifying {
        capture(p).await
    } else {
        let path = p.worktree_path.to_path_buf();
        tokio::task::spawn_blocking(move || surge_git::fingerprint::observe(&path))
            .await
            .ok()
            .and_then(Result::ok)
            .flatten()
            .zip(p.run_memory.verification.subject.as_ref())
            .map(|(observed, previous)| VerificationSubject {
                repository: observed.repository,
                worktree: observed.worktree,
                tree: observed.tree,
                checkpoint: previous.checkpoint.clone(),
            })
    };
    p.writer
        .append_event(VersionedEventPayload::new(
            EventPayload::VerificationSubjectObserved {
                subject: subject.clone(),
            },
        ))
        .await
        .map_err(|error| StageError::Storage(error.to_string()))?;
    let Some(task_id) = &p.active_task_id else {
        return Ok(None);
    };
    let mut criteria = accepted_criteria(p).await;
    if let Some(criteria) = &mut criteria {
        criteria.epoch = p
            .run_memory
            .verification
            .criteria
            .get(task_id)
            .filter(|previous| {
                subject
                    .as_ref()
                    .zip(p.run_memory.verification.subject.as_ref())
                    .is_some_and(|(captured, previous)| captured.same_code(previous))
                    && previous.epoch != surge_core::id::StageGenerationId::nil()
                    && previous.hash == criteria.hash
                    && previous.required == criteria.required
                    && previous.definitions == criteria.definitions
            })
            .map_or_else(surge_core::id::StageGenerationId::new, |previous| {
                previous.epoch
            });
    }
    p.writer
        .append_event(VersionedEventPayload::new(
            EventPayload::VerificationCriteriaAccepted {
                task_id: task_id.clone(),
                criteria: criteria.clone(),
            },
        ))
        .await
        .map_err(|error| StageError::Storage(error.to_string()))?;
    Ok(subject
        .zip(criteria)
        .map(|(subject, criteria)| VerificationBinding { subject, criteria }))
}

async fn capture(p: &AgentStageParams<'_>) -> Option<VerificationSubject> {
    let path = p.worktree_path.to_path_buf();
    let run = p.run_id;
    let seq = p.writer.current_seq().await.ok()?.as_u64();
    tokio::task::spawn_blocking(move || {
        let observation = surge_git::fingerprint::observe(&path).ok()??;
        let checkpoint = surge_git::checkpoint::capture_record(&path, run, seq).ok()??;
        let tree = surge_git::checkpoint::tree_identity(&checkpoint).ok()?;
        if observation.tree != tree { tracing::warn!("checkpoint filters differ from read-only workspace identity; verification unavailable"); return None; }
        Some(VerificationSubject { repository: observation.repository, worktree: observation.worktree, tree, checkpoint: checkpoint.commit })
    }).await.ok().flatten()
}

async fn accepted_criteria(p: &AgentStageParams<'_>) -> Option<VerificationCriteria> {
    accepted_contract(
        p.frames,
        p.run_memory,
        p.worktree_path,
        p.active_task_id.as_ref()?,
    )
    .await
}

async fn accepted_contract(
    frames: &[crate::engine::frames::Frame],
    memory: &surge_core::run_state::RunMemory,
    worktree: &std::path::Path,
    task_id: &surge_core::roadmap::RoadmapTaskId,
) -> Option<VerificationCriteria> {
    use crate::engine::frames::Frame;
    let item = frames.iter().rev().find_map(|frame| match frame {
        Frame::Loop(frame) => frame
            .items
            .get(frame.current_index as usize)
            .filter(|item| item.get("id").and_then(toml::Value::as_str) == Some(task_id.as_str())),
        Frame::Subgraph(_) => None,
    })?;
    let frozen: RoadmapTask = item.clone().try_into().ok()?;
    // A newer accepted amendment takes precedence over frozen iteration inputs.
    let amended = memory
        .roadmap_patches
        .values()
        .filter_map(|patch| {
            Some((
                patch.updated_seq,
                patch.roadmap_artifact?,
                patch.roadmap_path.clone()?,
            ))
        })
        .max_by_key(|entry| entry.0);
    let base = frames.iter().rev().find_map(|frame| {
        let Frame::Loop(frame) = frame else {
            return None;
        };
        match &frame.config.iterates_over {
            surge_core::loop_config::IterableSource::Artifact { name, .. }
            | surge_core::loop_config::IterableSource::RunArtifact { name, .. } => memory
                .artifacts
                .get(name)
                .map(|artifact| (artifact.hash, artifact.path.clone())),
            _ => None,
        }
    });
    let source = amended.map(|(_, hash, path)| (hash, path)).or(base);
    let Some((hash, path)) = source else {
        return VerificationCriteria::from_task(&frozen, &[]).ok();
    };
    let bytes = tokio::fs::read(worktree.join(path)).await.ok()?;
    if ContentHash::compute(&bytes) != hash {
        return None;
    }
    let roadmap: RoadmapArtifact = toml::from_str(std::str::from_utf8(&bytes).ok()?).ok()?;
    let tasks: Vec<_> = roadmap
        .milestones
        .iter()
        .flat_map(|milestone| &milestone.tasks)
        .filter(|task| &task.id == task_id)
        .collect();
    if tasks.len() != 1 {
        return None;
    }
    let assertions: Vec<_> = roadmap
        .missions
        .iter()
        .flat_map(|mission| mission.validation_contract.iter().cloned())
        .collect();
    VerificationCriteria::from_task(tasks[0], &assertions).ok()
}

pub(super) async fn seal(
    p: &AgentStageParams<'_>,
    binding: Option<&VerificationBinding>,
    report: Option<VerificationReportArtifact>,
    input_seq: surge_persistence::runs::EventSeq,
) -> Result<VerificationReportArtifact, String> {
    if p.active_task_id.is_none() {
        return unbound_audit(report.ok_or("generic verification requires an audit report")?);
    }
    let current_seq = p
        .writer
        .current_seq()
        .await
        .map_err(|error| error.to_string())?;
    let changes = p
        .writer
        .read_events(input_seq.next()..current_seq.next())
        .await
        .map_err(|error| error.to_string())?;
    let binding = binding.ok_or("verification input or accepted criteria unavailable")?;
    let task_id = p
        .active_task_id
        .as_ref()
        .ok_or("verification requires an active task")?;
    let mut context = surge_core::verification_evidence::VerificationContext {
        subject: Some(binding.subject.clone()),
        criteria: std::collections::BTreeMap::from([(task_id.clone(), binding.criteria.clone())]),
    };
    for event in changes {
        let invalidation = context.observe(event.payload.payload());
        if matches!(
            invalidation,
            surge_core::verification_evidence::VerificationInvalidation::All
        ) || matches!(invalidation,surge_core::verification_evidence::VerificationInvalidation::Task(ref id) if id==task_id)
        {
            return Err("verification input was invalidated during the attempt".into());
        }
    }
    let mut report = report.ok_or("Verified requires an inline verification report; files or synthetic hashes are not evidence")?;
    let task_id = p
        .active_task_id
        .as_ref()
        .ok_or("verification requires an active task")?;
    if report.task_id != task_id.as_str()
        || report.outcome != VerificationReportOutcome::Passed
        || report.binding.is_some()
    {
        return Err("report task/outcome mismatch or agent-supplied host binding".into());
    }
    let store = tokio::fs::canonicalize(p.artifact_store.root())
        .await
        .map_err(|error| format!("artifact store ownership unavailable: {error}"))?;
    if store.starts_with(&binding.subject.worktree) {
        return Err("sealed reports require artifact storage outside the checked worktree".into());
    }
    report.binding = Some(binding.clone());
    let report_text = toml::to_string(&report).map_err(|error| error.to_string())?;
    let validation = surge_core::artifact_contract::validate_artifact(
        surge_core::ArtifactKind::VerificationReport,
        None,
        &report_text,
    );
    if !validation.is_valid() || !report.covers(&binding.criteria.required) {
        return Err(
            "all required criteria need passing checks; skipped/cancelled checks cannot pass"
                .into(),
        );
    }
    let path = p.worktree_path.to_path_buf();
    let observed = tokio::task::spawn_blocking(move || surge_git::fingerprint::observe(&path))
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?
        .ok_or("workspace identity unavailable")?;
    if observed.repository != binding.subject.repository
        || observed.worktree != binding.subject.worktree
        || observed.tree != binding.subject.tree
    {
        return Err("workspace changed during verification; re-verification required".into());
    }
    let current = accepted_criteria(p)
        .await
        .ok_or("accepted criteria unavailable")?;
    if current.hash != binding.criteria.hash {
        return Err("accepted criteria changed during verification".into());
    }
    Ok(report)
}

pub(crate) async fn observe_after(
    writer: &surge_persistence::runs::run_writer::RunWriter,
    path: &std::path::Path,
    previous: Option<&VerificationSubject>,
) -> Result<(), StageError> {
    let path = path.to_path_buf();
    let observed = tokio::task::spawn_blocking(move || surge_git::fingerprint::observe(&path))
        .await
        .ok()
        .and_then(Result::ok)
        .flatten();
    let subject = observed
        .zip(previous)
        .map(|(observation, previous)| VerificationSubject {
            repository: observation.repository,
            worktree: observation.worktree,
            tree: observation.tree,
            checkpoint: previous.checkpoint.clone(),
        });
    writer
        .append_event(VersionedEventPayload::new(
            EventPayload::VerificationSubjectObserved { subject },
        ))
        .await
        .map_err(|error| StageError::Storage(error.to_string()))?;
    Ok(())
}

pub(super) fn seal_failure(
    p: &AgentStageParams<'_>,
    binding: Option<&VerificationBinding>,
    mut report: VerificationReportArtifact,
) -> Result<VerificationReportArtifact, String> {
    if report.outcome != VerificationReportOutcome::Failed
        || p.active_task_id
            .as_ref()
            .is_some_and(|task| task.as_str() != report.task_id)
        || report.binding.is_some()
    {
        return Err("failed report task/outcome mismatch or agent-supplied binding".into());
    }
    report.binding = binding.cloned();
    let report_text = toml::to_string(&report).map_err(|error| error.to_string())?;
    if !surge_core::artifact_contract::validate_artifact(
        surge_core::ArtifactKind::VerificationReport,
        None,
        &report_text,
    )
    .is_valid()
    {
        return Err("invalid failure report".into());
    }
    Ok(report)
}

fn unbound_audit(report: VerificationReportArtifact) -> Result<VerificationReportArtifact, String> {
    if report.binding.is_some() || report.outcome != VerificationReportOutcome::Passed {
        return Err("generic audit must be passed and cannot supply host binding".into());
    }
    let report_text = toml::to_string(&report).map_err(|error| error.to_string())?;
    if !surge_core::artifact_contract::validate_artifact(
        surge_core::ArtifactKind::VerificationReport,
        None,
        &report_text,
    )
    .is_valid()
    {
        return Err(
            "generic audit requires actual passing checks; skipped/cancelled checks cannot pass"
                .into(),
        );
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::frames::{Frame, LoopFrame};
    #[tokio::test]
    async fn accepted_amendment_overrides_frozen_task_and_resolves_assertions() {
        use surge_core::{
            loop_config::{ExitCondition, IterableSource, LoopConfig},
            roadmap::{RoadmapMilestone, RoadmapMission, ValidationAssertion},
            roadmap_patch::{RoadmapPatchStatus, RoadmapPatchTarget},
            run_state::RoadmapPatchMemory,
        };
        let root = tempfile::tempdir().unwrap();
        let mut task = RoadmapTask::new("t1", "implementation");
        task.acceptance_criteria = vec!["old criterion".into()];
        let frames = vec![Frame::Loop(LoopFrame {
            loop_node: "loop".parse().unwrap(),
            config: LoopConfig {
                iterates_over: IterableSource::Static(vec![]),
                body: "body".parse().unwrap(),
                iteration_var_name: "task".into(),
                exit_condition: ExitCondition::AllItems,
                on_iteration_failure: surge_core::loop_config::FailurePolicy::default(),
                parallelism: surge_core::loop_config::ParallelismMode::default(),
                gate_after_each: false,
            },
            items: vec![toml::Value::try_from(&task).unwrap()],
            current_index: 0,
            attempts_remaining: 0,
            return_to: "end".parse().unwrap(),
            traversal_counts: std::collections::HashMap::default(),
        })];
        let mut memory = surge_core::run_state::RunMemory::default();
        let old = accepted_contract(&frames, &memory, root.path(), &task.id)
            .await
            .unwrap();
        task.acceptance_criteria = vec!["new accepted criterion".into()];
        task.fulfills = vec!["VAL-001".into()];
        let mut milestone = RoadmapMilestone::new("m1", "delivery");
        milestone.tasks.push(task.clone());
        let mut roadmap = RoadmapArtifact::new(vec![milestone]);
        let mut mission = RoadmapMission::new("mission", "delivery", "goal");
        mission.validation_contract.push(ValidationAssertion {
            id: "VAL-001".into(),
            title: "behavior".into(),
            pass_condition: "observable accepted behavior".into(),
            evidence: vec!["test output".into()],
        });
        roadmap.missions.push(mission);
        let bytes = toml::to_string(&roadmap).unwrap();
        let path = root.path().join("accepted.toml");
        std::fs::write(&path, &bytes).unwrap();
        memory.roadmap_patches.insert(
            surge_core::roadmap_patch::RoadmapPatchId::new("accepted-update").unwrap(),
            RoadmapPatchMemory {
                target: RoadmapPatchTarget::ProjectRoadmap {
                    roadmap_path: "accepted.toml".into(),
                },
                status: RoadmapPatchStatus::Applied,
                patch_artifact: None,
                patch_path: None,
                roadmap_artifact: Some(ContentHash::compute(bytes.as_bytes())),
                roadmap_path: Some(path),
                flow_artifact: None,
                flow_path: None,
                updated_seq: 20,
            },
        );
        let current = accepted_contract(&frames, &memory, root.path(), &task.id)
            .await
            .unwrap();
        assert_ne!(old.hash, current.hash);
        assert_eq!(current.required, vec!["criterion:1", "assertion:VAL-001"]);
        assert_eq!(current.definitions[0].1, "new accepted criterion");
        roadmap.missions.clear();
        let bytes = toml::to_string(&roadmap).unwrap();
        let amendment = memory.roadmap_patches.values_mut().next().unwrap();
        std::fs::write(amendment.roadmap_path.as_ref().unwrap(), &bytes).unwrap();
        amendment.roadmap_artifact = Some(ContentHash::compute(bytes.as_bytes()));
        assert!(
            accepted_contract(&frames, &memory, root.path(), &task.id)
                .await
                .is_none(),
            "unresolved accepted assertion must fail closed"
        );
    }
}
