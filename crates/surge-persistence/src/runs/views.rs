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
        BootstrapStageStarted, EdgeTraversed, ForkCreated, HookExecuted, LoopCompleted,
        LoopIterationCompleted, LoopIterationStarted, OutcomeRejectedByHook, OutcomeReported,
        PipelineMaterialized, RunAborted, RunCompleted, RunFailed, RunStarted,
        SandboxElevationDecided, SandboxElevationRequested, SessionClosed, SessionOpened,
        StageCompleted, StageEntered, StageFailed, StageInputsResolved, TokensConsumed, ToolCalled,
        ToolResultReceived,
    };
    match payload {
        StageEntered { node, attempt } => {
            tx.execute(
                "INSERT INTO stage_executions (node_id, attempt, started_seq, started_at)
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
        _ => {}
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

/// Truncate all materialized view tables. Called at the start of `RebuildViews`.
pub fn rebuild(tx: &Transaction<'_>) -> Result<(), WriterError> {
    tx.execute_batch(
        "DELETE FROM stage_executions;
         DELETE FROM artifacts;
         DELETE FROM pending_approvals;
         DELETE FROM cost_summary;
         DELETE FROM graph_snapshots;",
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
    use surge_core::approvals::ApprovalChannel;
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
        use surge_core::approvals::ApprovalChannelKind;

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
        ] {
            let n: i64 = conn
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
                .unwrap();
            assert_eq!(n, 0, "table {table} should be empty");
        }
    }
}
