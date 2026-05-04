//! Engine snapshot — written at every stage boundary.

use serde::{Deserialize, Serialize};
use surge_core::keys::NodeKey;
use surge_core::run_state::Cursor;

/// Opaque blob persisted at every stage boundary so a crashed run can be
/// resumed without replaying the entire event log.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EngineSnapshot {
    /// Layout version; bump on any breaking schema change.
    pub schema_version: u32,
    /// Serializable form of the run cursor at the time of the snapshot.
    pub cursor: SerializableCursor,
    /// Event sequence number at which this snapshot was taken.
    pub at_seq: u64,
    /// Sequence number of the last completed stage boundary.
    pub stage_boundary_seq: u64,
    /// Non-`None` when the run was paused waiting for human input.
    pub pending_human_input: Option<PendingHumanInputSnapshot>,
}

/// Serde-friendly mirror of `surge_core::run_state::Cursor`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SerializableCursor {
    /// Serialized `NodeKey` (its inner string value).
    pub node: String,
    /// Attempt counter for retry tracking.
    pub attempt: u32,
}

impl From<&Cursor> for SerializableCursor {
    fn from(c: &Cursor) -> Self {
        Self {
            node: c.node.to_string(),
            attempt: c.attempt,
        }
    }
}

/// Errors that can occur when deserializing an `EngineSnapshot`.
#[derive(Debug, thiserror::Error)]
pub enum SnapshotError {
    /// The stored node key string is not a valid `NodeKey`.
    #[error("invalid node key in snapshot: {0}")]
    InvalidNodeKey(String),
}

impl SerializableCursor {
    /// Convert back to a `Cursor`, validating the stored node key string.
    pub fn into_cursor(self) -> Result<Cursor, SnapshotError> {
        Ok(Cursor {
            node: NodeKey::try_from(self.node.as_str())
                .map_err(|e| SnapshotError::InvalidNodeKey(format!("{}: {e}", self.node)))?,
            attempt: self.attempt,
        })
    }
}

/// Snapshot of a `HumanInputRequested` state persisted when the run pauses.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PendingHumanInputSnapshot {
    /// Serialized `NodeKey` of the node that requested input.
    pub node: String,
    /// ACP tool call identifier, if this was a tool-driven request.
    pub call_id: Option<String>,
    /// Human-readable prompt shown to the operator.
    pub prompt: String,
    /// Event sequence number when the request was emitted.
    pub requested_seq: u64,
}

impl EngineSnapshot {
    /// Current schema version. Bump on any breaking layout change.
    pub const SCHEMA_VERSION: u32 = 1;

    /// Create a new snapshot for the given cursor and sequence numbers.
    #[must_use]
    pub fn new(cursor: &Cursor, at_seq: u64, stage_boundary_seq: u64) -> Self {
        Self {
            schema_version: Self::SCHEMA_VERSION,
            cursor: SerializableCursor::from(cursor),
            at_seq,
            stage_boundary_seq,
            pending_human_input: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_via_json() {
        let cursor = Cursor {
            node: NodeKey::try_from("plan_1").unwrap(),
            attempt: 1,
        };
        let snap = EngineSnapshot::new(&cursor, 42, 41);
        let json = serde_json::to_vec(&snap).unwrap();
        let parsed: EngineSnapshot = serde_json::from_slice(&json).unwrap();
        assert_eq!(snap, parsed);
    }

    #[test]
    fn cursor_roundtrip_preserves_node_and_attempt() {
        let c = Cursor {
            node: NodeKey::try_from("agent_1").unwrap(),
            attempt: 3,
        };
        let s = SerializableCursor::from(&c);
        let back = s.into_cursor().unwrap();
        assert_eq!(back.node, c.node);
        assert_eq!(back.attempt, c.attempt);
    }
}
