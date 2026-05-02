//! Record types returned by view queries.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use surge_core::{ContentHash, NodeKey};

use crate::runs::seq::EventSeq;

/// One row of the `stage_executions` materialized view.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageExecution {
    /// Node identifier this attempt belongs to.
    pub node_id: NodeKey,
    /// 1-based attempt counter for this node.
    pub attempt: u32,
    /// Event seq at which the stage entered.
    pub started_seq: EventSeq,
    /// Event seq at which the stage ended, NULL while still running.
    pub ended_seq: Option<EventSeq>,
    /// Unix epoch ms of stage start.
    pub started_at_ms: i64,
    /// Unix epoch ms of stage end, NULL while still running.
    pub ended_at_ms: Option<i64>,
    /// Reported outcome key, NULL while still running or if no outcome was emitted.
    pub outcome: Option<String>,
    /// Cumulative cost in USD attributed to this attempt.
    pub cost_usd: f64,
    /// Cumulative input tokens for this attempt.
    pub tokens_in: u64,
    /// Cumulative output tokens for this attempt.
    pub tokens_out: u64,
}

/// One row of the `artifacts` materialized view.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactRecord {
    /// Primary key — content hash that addresses the artifact.
    pub id: ContentHash,
    /// Node that produced this artifact, NULL for bootstrap-stage artifacts.
    pub produced_by_node: Option<NodeKey>,
    /// Event seq at which the artifact was first recorded.
    pub produced_at_seq: EventSeq,
    /// Logical name (e.g., "spec.toml", "patch.diff").
    pub name: String,
    /// Path on disk relative to the artifacts directory.
    pub path: PathBuf,
    /// Length of the artifact bytes.
    pub size_bytes: u64,
    /// Content hash (same as `id` for the M2 schema).
    pub content_hash: ContentHash,
}

/// One row of the `pending_approvals` materialized view.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingApproval {
    /// Event seq at which the approval was requested.
    pub seq: EventSeq,
    /// Node that requested approval.
    pub node_id: NodeKey,
    /// Approval channel identifier (e.g., "telegram", "ui").
    pub channel: String,
    /// Unix epoch ms when the request was issued.
    pub requested_at_ms: i64,
    /// Hash of the payload that needs approval.
    pub payload_hash: String,
    /// Whether the request has been delivered to the channel.
    pub delivered: bool,
    /// External message id assigned by the channel after delivery, NULL otherwise.
    pub message_id: Option<i64>,
}

/// Summary of cost-related metrics from the `cost_summary` view.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct CostSummary {
    /// Total input tokens accumulated across the run.
    pub tokens_in: u64,
    /// Total output tokens accumulated across the run.
    pub tokens_out: u64,
    /// Total cache-hit tokens credited.
    pub cache_hits: u64,
    /// Total cost in USD accumulated across the run.
    pub cost_usd: f64,
}
