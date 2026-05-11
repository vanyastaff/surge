//! Engine-side materialized view maintenance.
//!
//! Called from the writer task inside the same transaction as the event INSERT.
//! Each `EventPayload` variant updates the affected view tables.
//!
//! View tables maintained:
//! - `stage_executions` ← `StageEntered`/`StageCompleted`/`StageFailed`
//! - `artifacts` ← `ArtifactProduced` (INSERT OR IGNORE — `StoreArtifact`
//!   command may have already inserted the row with the real `size_bytes`)
//! - `cost_summary` ← `TokensConsumed` (running totals via ON CONFLICT upsert)
//! - `pending_approvals` ← `ApprovalRequested`/`ApprovalDecided`
//!
//! All other event variants currently produce no view changes.

use rusqlite::Transaction;
use surge_core::roadmap_patch::{
    ActivePickupPolicy, OperatorConflictChoice, RoadmapPatchApprovalDecision, RoadmapPatchStatus,
    RoadmapPatchTarget,
};
use surge_core::run_event::EventPayload;

use crate::runs::error::WriterError;
use crate::runs::seq::EventSeq;

/// Update materialized view tables in response to a single event being appended.
///
/// Runs inside the same transaction as the originating `INSERT INTO events`.
pub fn maintain(
    tx: &Transaction<'_>,
    seq: EventSeq,
    timestamp_ms: i64,
    payload: &EventPayload,
) -> Result<(), WriterError> {
    use EventPayload::{
        ApprovalDecided, ApprovalRequested, ArtifactProduced, BootstrapApprovalDecided,
        BootstrapApprovalRequested, BootstrapArtifactProduced, BootstrapEditRequested,
        BootstrapStageStarted, EdgeTraversed, ForkCreated, GraphRevisionAccepted, HookExecuted,
        LoopCompleted, LoopIterationCompleted, LoopIterationStarted, OutcomeRejectedByHook,
        OutcomeReported, PipelineMaterialized, RoadmapPatchApplied, RoadmapPatchApprovalDecided,
        RoadmapPatchApprovalRequested, RoadmapPatchDrafted, RoadmapUpdated, RunAborted,
        RunCompleted, RunFailed, RunStarted, SandboxElevationDecided, SandboxElevationRequested,
        SessionClosed, SessionOpened, StageCompleted, StageEntered, StageFailed,
        StageInputsResolved, TokensConsumed, ToolCalled, ToolResultReceived,
    };
    match payload {
        StageEntered { node, attempt } => {
            // INSERT OR IGNORE — Loop body nodes re-enter the same (node_id, attempt)
            // on each iteration (M6). The first entry's data is preserved; subsequent
            // iterations for the same node are elided from this view. A full per-iteration
            // analytics row requires adding a `loop_iteration` column to the primary key
            // (deferred to M7 schema migration).
            tx.execute(
                "INSERT OR IGNORE INTO stage_executions (node_id, attempt, started_seq, started_at)
                 VALUES (?, ?, ?, ?)",
                rusqlite::params![
                    node.as_str(),
                    i64::from(*attempt),
                    seq.0 as i64,
                    timestamp_ms,
                ],
            )?;
        },
        StageCompleted { node, outcome } => {
            tx.execute(
                "UPDATE stage_executions
                 SET ended_seq = ?, ended_at = ?, outcome = ?
                 WHERE node_id = ?
                   AND attempt = (SELECT MAX(attempt) FROM stage_executions WHERE node_id = ?)",
                rusqlite::params![
                    seq.0 as i64,
                    timestamp_ms,
                    outcome.as_str(),
                    node.as_str(),
                    node.as_str(),
                ],
            )?;
        },
        StageFailed { node, .. } => {
            tx.execute(
                "UPDATE stage_executions
                 SET ended_seq = ?, ended_at = ?, outcome = NULL
                 WHERE node_id = ?
                   AND attempt = (SELECT MAX(attempt) FROM stage_executions WHERE node_id = ?)",
                rusqlite::params![seq.0 as i64, timestamp_ms, node.as_str(), node.as_str(),],
            )?;
        },
        ArtifactProduced {
            node,
            artifact,
            path,
            name,
        } => {
            // INSERT OR IGNORE — row may already exist from a prior `StoreArtifact`
            // command (which writes the real size_bytes from the on-disk content).
            // The event marks "produced"; the body was written then.
            tx.execute(
                "INSERT OR IGNORE INTO artifacts
                    (id, produced_by_node, produced_at_seq, name, path, size_bytes, content_hash)
                 VALUES (?, ?, ?, ?, ?, ?, ?)",
                rusqlite::params![
                    artifact.to_string(),
                    node.as_str(),
                    seq.0 as i64,
                    name,
                    path.to_string_lossy(),
                    0i64, // size unknown from event; StoreArtifact row has real size
                    artifact.to_string(),
                ],
            )?;
        },
        TokensConsumed {
            prompt_tokens,
            output_tokens,
            cache_hits,
            cost_usd,
            ..
        } => {
            upsert_metric(tx, "tokens_in", f64::from(*prompt_tokens), timestamp_ms)?;
            upsert_metric(tx, "tokens_out", f64::from(*output_tokens), timestamp_ms)?;
            upsert_metric(tx, "cache_hits", f64::from(*cache_hits), timestamp_ms)?;
            if let Some(cost) = cost_usd {
                upsert_metric(tx, "cost_usd", *cost, timestamp_ms)?;
            }
        },
        ApprovalRequested {
            gate,
            channel,
            payload_hash,
        } => {
            tx.execute(
                "INSERT INTO pending_approvals
                    (seq, node_id, channel, requested_at, payload_hash)
                 VALUES (?, ?, ?, ?, ?)",
                rusqlite::params![
                    seq.0 as i64,
                    gate.as_str(),
                    channel.kind().as_str(),
                    timestamp_ms,
                    payload_hash.to_string(),
                ],
            )?;
        },
        ApprovalDecided { gate, .. } => {
            tx.execute(
                "DELETE FROM pending_approvals WHERE node_id = ?",
                rusqlite::params![gate.as_str()],
            )?;
        },
        RoadmapPatchDrafted {
            patch_id,
            target,
            patch_artifact,
            patch_path,
        } => {
            let target_json = target_json(target)?;
            tx.execute(
                "INSERT INTO roadmap_patches
                    (patch_id, target_json, status, patch_artifact, patch_path,
                     created_seq, updated_seq, created_at, updated_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
                 ON CONFLICT(patch_id) DO UPDATE SET
                    target_json = excluded.target_json,
                    status = excluded.status,
                    patch_artifact = excluded.patch_artifact,
                    patch_path = excluded.patch_path,
                    summary_hash = NULL,
                    decision = NULL,
                    decision_comment = NULL,
                    conflict_choice = NULL,
                    amended_roadmap_artifact = NULL,
                    amended_roadmap_path = NULL,
                    amended_flow_artifact = NULL,
                    amended_flow_path = NULL,
                    roadmap_artifact = NULL,
                    roadmap_path = NULL,
                    flow_artifact = NULL,
                    flow_path = NULL,
                    active_pickup = NULL,
                    updated_seq = excluded.updated_seq,
                    updated_at = excluded.updated_at",
                rusqlite::params![
                    patch_id.as_str(),
                    target_json,
                    status_label(RoadmapPatchStatus::Drafted),
                    patch_artifact.to_string(),
                    patch_path.to_string_lossy(),
                    seq.0 as i64,
                    seq.0 as i64,
                    timestamp_ms,
                    timestamp_ms,
                ],
            )?;
        },
        RoadmapPatchApprovalRequested {
            patch_id,
            target,
            summary_hash,
            ..
        } => {
            let target_json = target_json(target)?;
            tx.execute(
                "UPDATE roadmap_patches
                 SET target_json = ?,
                     status = ?,
                     summary_hash = ?,
                     decision = NULL,
                     decision_comment = NULL,
                     conflict_choice = NULL,
                     amended_roadmap_artifact = NULL,
                     amended_roadmap_path = NULL,
                     amended_flow_artifact = NULL,
                     amended_flow_path = NULL,
                     roadmap_artifact = NULL,
                     roadmap_path = NULL,
                     flow_artifact = NULL,
                     flow_path = NULL,
                     active_pickup = NULL,
                     updated_seq = ?,
                     updated_at = ?
                 WHERE patch_id = ?",
                rusqlite::params![
                    target_json,
                    status_label(RoadmapPatchStatus::PendingApproval),
                    summary_hash.to_string(),
                    seq.0 as i64,
                    timestamp_ms,
                    patch_id.as_str(),
                ],
            )?;
        },
        RoadmapPatchApprovalDecided {
            patch_id,
            decision,
            comment,
            conflict_choice,
            ..
        } => {
            tx.execute(
                "UPDATE roadmap_patches
                 SET status = ?,
                     decision = ?,
                     decision_comment = ?,
                     conflict_choice = ?,
                     updated_seq = ?,
                     updated_at = ?
                 WHERE patch_id = ?",
                rusqlite::params![
                    status_label(status_for_decision(*decision)),
                    decision_label(*decision),
                    comment,
                    conflict_choice.map(choice_label),
                    seq.0 as i64,
                    timestamp_ms,
                    patch_id.as_str(),
                ],
            )?;
        },
        RoadmapPatchApplied {
            patch_id,
            target,
            amended_roadmap_artifact,
            amended_roadmap_path,
            amended_flow_artifact,
            amended_flow_path,
        } => {
            let target_json = target_json(target)?;
            tx.execute(
                "INSERT INTO roadmap_patches
                    (patch_id, target_json, status,
                     amended_roadmap_artifact, amended_roadmap_path,
                     amended_flow_artifact, amended_flow_path,
                     created_seq, updated_seq, created_at, updated_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                 ON CONFLICT(patch_id) DO UPDATE SET
                    target_json = excluded.target_json,
                    status = excluded.status,
                    amended_roadmap_artifact = excluded.amended_roadmap_artifact,
                    amended_roadmap_path = excluded.amended_roadmap_path,
                    amended_flow_artifact = excluded.amended_flow_artifact,
                    amended_flow_path = excluded.amended_flow_path,
                    updated_seq = excluded.updated_seq,
                    updated_at = excluded.updated_at",
                rusqlite::params![
                    patch_id.as_str(),
                    target_json,
                    status_label(RoadmapPatchStatus::Applied),
                    amended_roadmap_artifact.to_string(),
                    amended_roadmap_path.to_string_lossy(),
                    amended_flow_artifact.map(|hash| hash.to_string()),
                    amended_flow_path
                        .as_ref()
                        .map(|path| path.to_string_lossy().to_string()),
                    seq.0 as i64,
                    seq.0 as i64,
                    timestamp_ms,
                    timestamp_ms,
                ],
            )?;
        },
        RoadmapUpdated {
            patch_id,
            target,
            roadmap_artifact,
            roadmap_path,
            flow_artifact,
            flow_path,
            active_pickup,
        } => {
            let target_json = target_json(target)?;
            tx.execute(
                "INSERT INTO roadmap_patches
                    (patch_id, target_json, status,
                     roadmap_artifact, roadmap_path, flow_artifact, flow_path,
                     active_pickup, created_seq, updated_seq, created_at, updated_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                 ON CONFLICT(patch_id) DO UPDATE SET
                    target_json = excluded.target_json,
                    status = excluded.status,
                    roadmap_artifact = excluded.roadmap_artifact,
                    roadmap_path = excluded.roadmap_path,
                    flow_artifact = excluded.flow_artifact,
                    flow_path = excluded.flow_path,
                    active_pickup = excluded.active_pickup,
                    updated_seq = excluded.updated_seq,
                    updated_at = excluded.updated_at",
                rusqlite::params![
                    patch_id.as_str(),
                    target_json,
                    status_label(RoadmapPatchStatus::Applied),
                    roadmap_artifact.to_string(),
                    roadmap_path.to_string_lossy(),
                    flow_artifact.map(|hash| hash.to_string()),
                    flow_path
                        .as_ref()
                        .map(|path| path.to_string_lossy().to_string()),
                    pickup_label(*active_pickup),
                    seq.0 as i64,
                    seq.0 as i64,
                    timestamp_ms,
                    timestamp_ms,
                ],
            )?;
        },
        // All other variants currently produce no view changes.
        // Listed explicitly here so a future variant addition forces a
        // conscious decision rather than silently falling through.
        RunStarted { .. }
        | RunCompleted { .. }
        | RunFailed { .. }
        | RunAborted { .. }
        | BootstrapStageStarted { .. }
        | BootstrapArtifactProduced { .. }
        | BootstrapApprovalRequested { .. }
        | BootstrapApprovalDecided { .. }
        | BootstrapEditRequested { .. }
        | PipelineMaterialized { .. }
        | GraphRevisionAccepted { .. }
        | StageInputsResolved { .. }
        | SessionOpened { .. }
        | ToolCalled { .. }
        | ToolResultReceived { .. }
        | OutcomeReported { .. }
        | SessionClosed { .. }
        | EdgeTraversed { .. }
        | LoopIterationStarted { .. }
        | LoopIterationCompleted { .. }
        | LoopCompleted { .. }
        | SandboxElevationRequested { .. }
        | SandboxElevationDecided { .. }
        | HookExecuted { .. }
        | OutcomeRejectedByHook { .. }
        | ForkCreated { .. } => {},
        // M5 HumanInput variants — not aggregated into materialized views.
        _ => {},
    }
    Ok(())
}

/// Upsert a single metric row in `cost_summary`, accumulating into `value`.
fn upsert_metric(tx: &Transaction<'_>, metric: &str, delta: f64, ts: i64) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT INTO cost_summary (metric, value, updated_at)
         VALUES (?, ?, ?)
         ON CONFLICT(metric) DO UPDATE SET
             value = value + excluded.value,
             updated_at = excluded.updated_at",
        rusqlite::params![metric, delta, ts],
    )?;
    Ok(())
}

fn target_json(target: &RoadmapPatchTarget) -> Result<String, WriterError> {
    serde_json::to_string(target).map_err(Into::into)
}

const fn status_for_decision(decision: RoadmapPatchApprovalDecision) -> RoadmapPatchStatus {
    match decision {
        RoadmapPatchApprovalDecision::Approve => RoadmapPatchStatus::Approved,
        RoadmapPatchApprovalDecision::Edit => RoadmapPatchStatus::Drafted,
        RoadmapPatchApprovalDecision::Reject => RoadmapPatchStatus::Rejected,
    }
}

const fn status_label(status: RoadmapPatchStatus) -> &'static str {
    match status {
        RoadmapPatchStatus::Drafted => "drafted",
        RoadmapPatchStatus::PendingApproval => "pending_approval",
        RoadmapPatchStatus::Approved => "approved",
        RoadmapPatchStatus::Applied => "applied",
        RoadmapPatchStatus::Rejected => "rejected",
        RoadmapPatchStatus::Superseded => "superseded",
    }
}

const fn decision_label(decision: RoadmapPatchApprovalDecision) -> &'static str {
    match decision {
        RoadmapPatchApprovalDecision::Approve => "approve",
        RoadmapPatchApprovalDecision::Edit => "edit",
        RoadmapPatchApprovalDecision::Reject => "reject",
    }
}

const fn choice_label(choice: OperatorConflictChoice) -> &'static str {
    match choice {
        OperatorConflictChoice::DeferToNextMilestone => "defer_to_next_milestone",
        OperatorConflictChoice::AbortCurrentRun => "abort_current_run",
        OperatorConflictChoice::CreateFollowUpRun => "create_follow_up_run",
        OperatorConflictChoice::RejectPatch => "reject_patch",
    }
}

const fn pickup_label(policy: ActivePickupPolicy) -> &'static str {
    match policy {
        ActivePickupPolicy::Allowed => "allowed",
        ActivePickupPolicy::FollowUpOnly => "follow_up_only",
        ActivePickupPolicy::Disabled => "disabled",
    }
}

/// Truncate all materialized view tables. Called at the start of `RebuildViews`.
pub fn rebuild(tx: &Transaction<'_>) -> Result<(), WriterError> {
    tx.execute_batch(
        "DELETE FROM stage_executions;
         DELETE FROM artifacts;
         DELETE FROM pending_approvals;
         DELETE FROM cost_summary;
         DELETE FROM graph_snapshots;
         DELETE FROM roadmap_patches;",
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runs::clock::MockClock;
    use crate::runs::migrations::{PER_RUN_MIGRATIONS, apply as apply_migrations};
    use rusqlite::Connection;
    use std::path::PathBuf;
    use std::str::FromStr;
    use surge_core::approvals::{ApprovalChannel, ApprovalChannelKind, ApprovalDuration};
    use surge_core::roadmap_patch::{
        ActivePickupPolicy, RoadmapPatchApprovalDecision, RoadmapPatchId, RoadmapPatchTarget,
    };
    use surge_core::run_event::EventPayload;
    use surge_core::{ContentHash, NodeKey, OutcomeKey, SessionId};

    fn fresh_db() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        let clock = MockClock::new(1_700_000_000_000);
        apply_migrations(&mut conn, PER_RUN_MIGRATIONS, &clock).unwrap();
        conn
    }

    fn n(s: &str) -> NodeKey {
        NodeKey::from_str(s).unwrap()
    }

    fn o(s: &str) -> OutcomeKey {
        OutcomeKey::from_str(s).unwrap()
    }

    #[test]
    fn stage_entered_inserts_row() {
        let mut conn = fresh_db();
        let tx = conn.transaction().unwrap();
        maintain(
            &tx,
            EventSeq(1),
            1_700_000_000_001,
            &EventPayload::StageEntered {
                node: n("spec_1"),
                attempt: 1,
            },
        )
        .unwrap();
        tx.commit().unwrap();

        let (attempt, started_seq, started_at, ended_seq, outcome): (
            i64,
            i64,
            i64,
            Option<i64>,
            Option<String>,
        ) = conn
            .query_row(
                "SELECT attempt, started_seq, started_at, ended_seq, outcome
                 FROM stage_executions WHERE node_id = ?",
                rusqlite::params!["spec_1"],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, i64>(2)?,
                        r.get::<_, Option<i64>>(3)?,
                        r.get::<_, Option<String>>(4)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(attempt, 1);
        assert_eq!(started_seq, 1);
        assert_eq!(started_at, 1_700_000_000_001);
        assert_eq!(ended_seq, None);
        assert_eq!(outcome, None);
    }

    #[test]
    fn stage_entered_then_completed_updates_row() {
        let mut conn = fresh_db();
        let tx = conn.transaction().unwrap();
        let node = n("impl_1");
        maintain(
            &tx,
            EventSeq(1),
            1_700_000_000_001,
            &EventPayload::StageEntered {
                node: node.clone(),
                attempt: 1,
            },
        )
        .unwrap();
        maintain(
            &tx,
            EventSeq(2),
            1_700_000_000_002,
            &EventPayload::StageCompleted {
                node: node.clone(),
                outcome: o("done"),
            },
        )
        .unwrap();
        tx.commit().unwrap();

        let (started_seq, ended_seq, ended_at, outcome): (
            i64,
            Option<i64>,
            Option<i64>,
            Option<String>,
        ) = conn
            .query_row(
                "SELECT started_seq, ended_seq, ended_at, outcome
                 FROM stage_executions WHERE node_id = ?",
                rusqlite::params!["impl_1"],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(started_seq, 1);
        assert_eq!(ended_seq, Some(2));
        assert_eq!(ended_at, Some(1_700_000_000_002));
        assert_eq!(outcome, Some("done".to_string()));
    }

    #[test]
    fn stage_completed_targets_max_attempt() {
        let mut conn = fresh_db();
        let tx = conn.transaction().unwrap();
        let node = n("impl_1");

        // Two attempts entered, both end without explicit completion.
        maintain(
            &tx,
            EventSeq(1),
            1_700_000_000_001,
            &EventPayload::StageEntered {
                node: node.clone(),
                attempt: 1,
            },
        )
        .unwrap();
        maintain(
            &tx,
            EventSeq(2),
            1_700_000_000_002,
            &EventPayload::StageEntered {
                node: node.clone(),
                attempt: 2,
            },
        )
        .unwrap();
        // Completion event refers to the most recent attempt.
        maintain(
            &tx,
            EventSeq(3),
            1_700_000_000_003,
            &EventPayload::StageCompleted {
                node: node.clone(),
                outcome: o("done"),
            },
        )
        .unwrap();
        tx.commit().unwrap();

        // Attempt 1 must remain open; attempt 2 must be closed.
        let (a1_ended, a2_ended): (Option<i64>, Option<i64>) = (
            conn.query_row(
                "SELECT ended_seq FROM stage_executions WHERE node_id = ? AND attempt = 1",
                rusqlite::params!["impl_1"],
                |r| r.get(0),
            )
            .unwrap(),
            conn.query_row(
                "SELECT ended_seq FROM stage_executions WHERE node_id = ? AND attempt = 2",
                rusqlite::params!["impl_1"],
                |r| r.get(0),
            )
            .unwrap(),
        );
        assert_eq!(a1_ended, None);
        assert_eq!(a2_ended, Some(3));
    }

    #[test]
    fn stage_failed_clears_outcome_on_max_attempt() {
        let mut conn = fresh_db();
        let tx = conn.transaction().unwrap();
        let node = n("impl_1");
        maintain(
            &tx,
            EventSeq(1),
            1_700_000_000_001,
            &EventPayload::StageEntered {
                node: node.clone(),
                attempt: 1,
            },
        )
        .unwrap();
        maintain(
            &tx,
            EventSeq(2),
            1_700_000_000_002,
            &EventPayload::StageFailed {
                node: node.clone(),
                reason: "boom".into(),
                retry_available: true,
            },
        )
        .unwrap();
        tx.commit().unwrap();

        let (ended_seq, outcome): (Option<i64>, Option<String>) = conn
            .query_row(
                "SELECT ended_seq, outcome FROM stage_executions WHERE node_id = ?",
                rusqlite::params!["impl_1"],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(ended_seq, Some(2));
        assert_eq!(outcome, None);
    }

    #[test]
    fn artifact_produced_inserts_row() {
        let mut conn = fresh_db();
        let tx = conn.transaction().unwrap();
        let hash = ContentHash::compute(b"data");
        maintain(
            &tx,
            EventSeq(7),
            1_700_000_000_005,
            &EventPayload::ArtifactProduced {
                node: n("spec_1"),
                artifact: hash,
                path: PathBuf::from("artifacts/spec.md"),
                name: "spec.md".into(),
            },
        )
        .unwrap();
        tx.commit().unwrap();

        let (id, produced_by, produced_at_seq, name, path, size_bytes, content_hash): (
            String,
            Option<String>,
            i64,
            String,
            String,
            i64,
            String,
        ) = conn
            .query_row(
                "SELECT id, produced_by_node, produced_at_seq, name, path, size_bytes, content_hash
                 FROM artifacts WHERE id = ?",
                rusqlite::params![hash.to_string()],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(id, hash.to_string());
        assert_eq!(produced_by, Some("spec_1".to_string()));
        assert_eq!(produced_at_seq, 7);
        assert_eq!(name, "spec.md");
        assert!(path.contains("spec.md"), "got path {path}");
        assert_eq!(size_bytes, 0);
        assert_eq!(content_hash, hash.to_string());
    }

    #[test]
    fn artifact_produced_does_not_overwrite_existing_row() {
        let mut conn = fresh_db();
        let hash = ContentHash::compute(b"data");

        // Simulate the prior `StoreArtifact` row with the real size_bytes.
        conn.execute(
            "INSERT INTO artifacts
                (id, produced_by_node, produced_at_seq, name, path, size_bytes, content_hash)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
            rusqlite::params![
                hash.to_string(),
                "spec_1",
                7i64,
                "spec.md",
                "artifacts/spec.md",
                12345i64,
                hash.to_string(),
            ],
        )
        .unwrap();

        let tx = conn.transaction().unwrap();
        maintain(
            &tx,
            EventSeq(7),
            1_700_000_000_005,
            &EventPayload::ArtifactProduced {
                node: n("spec_1"),
                artifact: hash,
                path: PathBuf::from("artifacts/spec.md"),
                name: "spec.md".into(),
            },
        )
        .unwrap();
        tx.commit().unwrap();

        let size: i64 = conn
            .query_row(
                "SELECT size_bytes FROM artifacts WHERE id = ?",
                rusqlite::params![hash.to_string()],
                |r| r.get(0),
            )
            .unwrap();
        // INSERT OR IGNORE preserved the prior size; did not zero it.
        assert_eq!(size, 12345);
    }

    #[test]
    fn tokens_consumed_accumulates() {
        let mut conn = fresh_db();
        let tx = conn.transaction().unwrap();
        maintain(
            &tx,
            EventSeq(1),
            1_700_000_000_001,
            &EventPayload::TokensConsumed {
                session: SessionId::new(),
                prompt_tokens: 100,
                output_tokens: 50,
                cache_hits: 10,
                model: "claude".into(),
                cost_usd: Some(0.01),
            },
        )
        .unwrap();
        maintain(
            &tx,
            EventSeq(2),
            1_700_000_000_002,
            &EventPayload::TokensConsumed {
                session: SessionId::new(),
                prompt_tokens: 200,
                output_tokens: 75,
                cache_hits: 5,
                model: "claude".into(),
                cost_usd: Some(0.02),
            },
        )
        .unwrap();
        tx.commit().unwrap();

        let mut rows: Vec<(String, f64)> = conn
            .prepare("SELECT metric, value FROM cost_summary ORDER BY metric")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        rows.sort_by(|a, b| a.0.cmp(&b.0));

        let map: std::collections::BTreeMap<_, _> = rows.into_iter().collect();
        assert_eq!(map.get("tokens_in").copied(), Some(300.0));
        assert_eq!(map.get("tokens_out").copied(), Some(125.0));
        assert_eq!(map.get("cache_hits").copied(), Some(15.0));
        assert!((map.get("cost_usd").copied().unwrap() - 0.03).abs() < 1e-9);
    }

    #[test]
    fn tokens_consumed_skips_cost_when_none() {
        let mut conn = fresh_db();
        let tx = conn.transaction().unwrap();
        maintain(
            &tx,
            EventSeq(1),
            1_700_000_000_001,
            &EventPayload::TokensConsumed {
                session: SessionId::new(),
                prompt_tokens: 10,
                output_tokens: 5,
                cache_hits: 0,
                model: "claude".into(),
                cost_usd: None,
            },
        )
        .unwrap();
        tx.commit().unwrap();

        let cost: Option<f64> = conn
            .query_row(
                "SELECT value FROM cost_summary WHERE metric = 'cost_usd'",
                [],
                |r| r.get(0),
            )
            .ok();
        assert_eq!(cost, None);
    }

    #[test]
    fn approval_requested_inserts_pending_row() {
        let mut conn = fresh_db();
        let tx = conn.transaction().unwrap();
        let hash = ContentHash::compute(b"summary");
        maintain(
            &tx,
            EventSeq(11),
            1_700_000_000_011,
            &EventPayload::ApprovalRequested {
                gate: n("gate_main"),
                channel: ApprovalChannel::Telegram {
                    chat_id_ref: "$DEFAULT".into(),
                },
                payload_hash: hash,
            },
        )
        .unwrap();
        tx.commit().unwrap();

        let (seq, node_id, channel, requested_at, payload_hash, delivered): (
            i64,
            String,
            String,
            i64,
            String,
            i64,
        ) = conn
            .query_row(
                "SELECT seq, node_id, channel, requested_at, payload_hash, delivered
                 FROM pending_approvals WHERE seq = 11",
                [],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(seq, 11);
        assert_eq!(node_id, "gate_main");
        assert_eq!(channel, "telegram");
        assert_eq!(requested_at, 1_700_000_000_011);
        assert_eq!(payload_hash, hash.to_string());
        assert_eq!(delivered, 0);
    }

    #[test]
    fn approval_requested_then_decided_clears_row() {
        let mut conn = fresh_db();
        let tx = conn.transaction().unwrap();
        maintain(
            &tx,
            EventSeq(11),
            1_700_000_000_011,
            &EventPayload::ApprovalRequested {
                gate: n("gate_main"),
                channel: ApprovalChannel::Telegram {
                    chat_id_ref: "$DEFAULT".into(),
                },
                payload_hash: ContentHash::compute(b"summary"),
            },
        )
        .unwrap();
        maintain(
            &tx,
            EventSeq(12),
            1_700_000_000_012,
            &EventPayload::ApprovalDecided {
                gate: n("gate_main"),
                decision: "approve".into(),
                channel_used: ApprovalChannelKind::Telegram,
                comment: None,
            },
        )
        .unwrap();
        tx.commit().unwrap();

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pending_approvals WHERE node_id = ?",
                rusqlite::params!["gate_main"],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn roadmap_patch_lifecycle_updates_view() {
        let mut conn = fresh_db();
        let tx = conn.transaction().unwrap();
        let patch_id = RoadmapPatchId::new("rpatch-view").unwrap();
        let target = RoadmapPatchTarget::ProjectRoadmap {
            roadmap_path: ".ai-factory/ROADMAP.md".into(),
        };
        let patch_hash = ContentHash::compute(b"patch");
        let roadmap_hash = ContentHash::compute(b"roadmap");
        let flow_hash = ContentHash::compute(b"flow");

        maintain(
            &tx,
            EventSeq(1),
            1_700_000_000_001,
            &EventPayload::RoadmapPatchDrafted {
                patch_id: patch_id.clone(),
                target: target.clone(),
                patch_artifact: patch_hash,
                patch_path: PathBuf::from("roadmap-patch.toml"),
            },
        )
        .unwrap();
        maintain(
            &tx,
            EventSeq(2),
            1_700_000_000_002,
            &EventPayload::RoadmapPatchApprovalRequested {
                patch_id: patch_id.clone(),
                target: target.clone(),
                channel: ApprovalChannel::Desktop {
                    duration: ApprovalDuration::Transient,
                },
                summary_hash: ContentHash::compute(b"summary"),
            },
        )
        .unwrap();
        maintain(
            &tx,
            EventSeq(3),
            1_700_000_000_003,
            &EventPayload::RoadmapPatchApprovalDecided {
                patch_id: patch_id.clone(),
                decision: RoadmapPatchApprovalDecision::Approve,
                channel_used: ApprovalChannelKind::Desktop,
                comment: Some("ok".into()),
                conflict_choice: None,
            },
        )
        .unwrap();
        maintain(
            &tx,
            EventSeq(4),
            1_700_000_000_004,
            &EventPayload::RoadmapUpdated {
                patch_id: patch_id.clone(),
                target,
                roadmap_artifact: roadmap_hash,
                roadmap_path: PathBuf::from("roadmap.toml"),
                flow_artifact: Some(flow_hash),
                flow_path: Some(PathBuf::from("flow.toml")),
                active_pickup: ActivePickupPolicy::Allowed,
            },
        )
        .unwrap();
        tx.commit().unwrap();

        let (status, patch_artifact, roadmap_artifact, flow_artifact, updated_seq): (
            String,
            String,
            String,
            String,
            i64,
        ) = conn
            .query_row(
                "SELECT status, patch_artifact, roadmap_artifact, flow_artifact, updated_seq
                 FROM roadmap_patches WHERE patch_id = ?",
                rusqlite::params![patch_id.as_str()],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .unwrap();

        assert_eq!(status, "applied");
        assert_eq!(patch_artifact, patch_hash.to_string());
        assert_eq!(roadmap_artifact, roadmap_hash.to_string());
        assert_eq!(flow_artifact, flow_hash.to_string());
        assert_eq!(updated_seq, 4);
    }

    #[test]
    fn roadmap_patch_draft_and_pending_clear_stale_lifecycle_fields() {
        let mut conn = fresh_db();
        let tx = conn.transaction().unwrap();
        let patch_id = RoadmapPatchId::new("rpatch-clear").unwrap();
        let target = RoadmapPatchTarget::ProjectRoadmap {
            roadmap_path: ".ai-factory/ROADMAP.md".into(),
        };
        let patch_hash = ContentHash::compute(b"patch");
        let next_patch_hash = ContentHash::compute(b"patch-next");

        maintain(
            &tx,
            EventSeq(1),
            1_700_000_000_001,
            &EventPayload::RoadmapPatchDrafted {
                patch_id: patch_id.clone(),
                target: target.clone(),
                patch_artifact: patch_hash,
                patch_path: PathBuf::from("roadmap-patch.toml"),
            },
        )
        .unwrap();
        maintain(
            &tx,
            EventSeq(2),
            1_700_000_000_002,
            &EventPayload::RoadmapPatchApprovalRequested {
                patch_id: patch_id.clone(),
                target: target.clone(),
                channel: ApprovalChannel::Desktop {
                    duration: ApprovalDuration::Transient,
                },
                summary_hash: ContentHash::compute(b"summary"),
            },
        )
        .unwrap();
        maintain(
            &tx,
            EventSeq(3),
            1_700_000_000_003,
            &EventPayload::RoadmapPatchApprovalDecided {
                patch_id: patch_id.clone(),
                decision: RoadmapPatchApprovalDecision::Approve,
                channel_used: ApprovalChannelKind::Desktop,
                comment: Some("ok".into()),
                conflict_choice: Some(OperatorConflictChoice::CreateFollowUpRun),
            },
        )
        .unwrap();
        maintain(
            &tx,
            EventSeq(4),
            1_700_000_000_004,
            &EventPayload::RoadmapUpdated {
                patch_id: patch_id.clone(),
                target: target.clone(),
                roadmap_artifact: ContentHash::compute(b"roadmap"),
                roadmap_path: PathBuf::from("roadmap.toml"),
                flow_artifact: Some(ContentHash::compute(b"flow")),
                flow_path: Some(PathBuf::from("flow.toml")),
                active_pickup: ActivePickupPolicy::Allowed,
            },
        )
        .unwrap();
        maintain(
            &tx,
            EventSeq(5),
            1_700_000_000_005,
            &EventPayload::RoadmapPatchDrafted {
                patch_id: patch_id.clone(),
                target: target.clone(),
                patch_artifact: next_patch_hash,
                patch_path: PathBuf::from("roadmap-patch-next.toml"),
            },
        )
        .unwrap();
        maintain(
            &tx,
            EventSeq(6),
            1_700_000_000_006,
            &EventPayload::RoadmapPatchApprovalRequested {
                patch_id: patch_id.clone(),
                target,
                channel: ApprovalChannel::Desktop {
                    duration: ApprovalDuration::Transient,
                },
                summary_hash: ContentHash::compute(b"summary-next"),
            },
        )
        .unwrap();
        tx.commit().unwrap();

        let (status, decision, conflict_choice, roadmap_artifact, flow_artifact): (
            String,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
        ) = conn
            .query_row(
                "SELECT status, decision, conflict_choice, roadmap_artifact, flow_artifact
                 FROM roadmap_patches WHERE patch_id = ?",
                rusqlite::params![patch_id.as_str()],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .unwrap();

        assert_eq!(status, "pending_approval");
        assert_eq!(decision, None);
        assert_eq!(conflict_choice, None);
        assert_eq!(roadmap_artifact, None);
        assert_eq!(flow_artifact, None);
    }

    #[test]
    fn no_op_variant_does_not_touch_views() {
        let mut conn = fresh_db();
        let tx = conn.transaction().unwrap();
        // RunCompleted is a no-op for view maintenance.
        maintain(
            &tx,
            EventSeq(99),
            1_700_000_000_099,
            &EventPayload::RunCompleted {
                terminal_node: n("end"),
            },
        )
        .unwrap();
        tx.commit().unwrap();

        for table in [
            "stage_executions",
            "artifacts",
            "pending_approvals",
            "cost_summary",
            "roadmap_patches",
        ] {
            let n: i64 = conn
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
                .unwrap();
            assert_eq!(n, 0, "table {table} should be empty");
        }
    }
}
