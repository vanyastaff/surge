//! Engine snapshot — written at every stage boundary.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
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
    /// Frame stack — empty for unnested execution. New in v2.
    #[serde(default)]
    pub frames: Vec<SerializableFrame>,
    /// Per-edge traversal counters for max_traversals enforcement
    /// outside loop frames. Map key is the `EdgeKey` as a string. New in v2.
    #[serde(default)]
    pub root_traversal_counts: HashMap<String, u32>,
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
    /// The blob is not valid JSON.
    #[error("invalid JSON: {0}")]
    InvalidJson(String),
    /// The blob does not include a `schema_version` field.
    #[error("snapshot is missing schema_version")]
    MissingSchemaVersion,
    /// The blob has a schema_version this engine doesn't support.
    #[error("unsupported snapshot schema version: {0:?}")]
    UnsupportedSchema(Option<u64>),
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

/// Serde-friendly mirror of [`crate::engine::frames::Frame`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SerializableFrame {
    /// Serialized form of a `Frame::Loop` entry.
    Loop(SerializableLoopFrame),
    /// Serialized form of a `Frame::Subgraph` entry.
    Subgraph(SerializableSubgraphFrame),
}

/// Serializable form of a [`crate::engine::frames::LoopFrame`].
///
/// `LoopConfig` is not stored as a single serialized blob because
/// `IterableSource::Static` embeds `toml::Value` inside an internally-tagged
/// serde enum, which neither `serde_json` nor `toml` can serialize.  Instead
/// we flatten the config fields we need to preserve across snapshots.
/// The resolved iteration items are already in `items_json`, so `iterates_over`
/// is only persisted for the `Artifact` path (stored as `iterable_source_json`).
/// For `Static`, `iterable_source_json` is `null` and items are reconstructed
/// from `items_json` in the Task 2.3 reverse conversion.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SerializableLoopFrame {
    /// Serialized `NodeKey` of the outer Loop node.
    pub loop_node: String,
    /// `IterableSource` stored as JSON, or `null` for `Static` (items are in
    /// `items_json`).
    pub iterable_source_json: Option<String>,
    /// Body subgraph key string.
    pub body: String,
    /// Variable name for the current item inside the loop body.
    pub iteration_var_name: String,
    /// Exit condition stored as JSON.
    pub exit_condition_json: String,
    /// Failure policy stored as JSON.
    pub on_iteration_failure_json: String,
    /// Parallelism mode stored as JSON.
    pub parallelism_json: String,
    /// Whether to insert a human-review gate after each iteration.
    pub gate_after_each: bool,
    /// Resolved iteration items (one per loop step).
    pub items_json: Vec<serde_json::Value>,
    /// 0-based index of the current iteration.
    pub current_index: u32,
    /// Remaining retry attempts for the current iteration.
    pub attempts_remaining: u32,
    /// Serialized `NodeKey` of the outer-graph node to resume after the loop.
    pub return_to: String,
    /// Per-edge traversal counters for body edges.
    pub traversal_counts: HashMap<String, u32>,
}

/// Serializable form of a [`crate::engine::frames::SubgraphFrame`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SerializableSubgraphFrame {
    /// Serialized `NodeKey` of the outer Subgraph node.
    pub outer_node: String,
    /// Serialized `SubgraphKey` referencing the inner subgraph.
    pub inner_subgraph: String,
    /// Resolved input bindings.
    pub bound_inputs: Vec<SerializableSubgraphInput>,
    /// Serialized `NodeKey` of the outer-graph node to resume after the subgraph.
    pub return_to: String,
}

/// One resolved subgraph input in serializable form.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SerializableSubgraphInput {
    /// Inner template variable name (the string inside `{{...}}`).
    pub inner_var: String,
    /// Resolved value.
    pub value: serde_json::Value,
}

impl From<crate::engine::frames::Frame> for SerializableFrame {
    fn from(f: crate::engine::frames::Frame) -> Self {
        match f {
            crate::engine::frames::Frame::Loop(lf) => {
                use surge_core::loop_config::IterableSource;
                // `IterableSource::Static` embeds `toml::Value` inside an
                // internally-tagged serde enum. Neither `serde_json` nor `toml`
                // can serialize a tagged newtype variant wrapping a sequence, so
                // the static items are stored separately in `items_json` and the
                // `iterable_source_json` field is `null` for that variant.
                let iterable_source_json = match &lf.config.iterates_over {
                    IterableSource::Static(_) => None,
                    src => Some(
                        serde_json::to_string(src)
                            .expect("IterableSource::Artifact is json-serializable"),
                    ),
                };
                Self::Loop(SerializableLoopFrame {
                    loop_node: lf.loop_node.to_string(),
                    iterable_source_json,
                    body: lf.config.body.to_string(),
                    iteration_var_name: lf.config.iteration_var_name.clone(),
                    exit_condition_json: serde_json::to_string(&lf.config.exit_condition)
                        .expect("ExitCondition is json-serializable"),
                    on_iteration_failure_json: serde_json::to_string(
                        &lf.config.on_iteration_failure,
                    )
                    .expect("FailurePolicy is json-serializable"),
                    parallelism_json: serde_json::to_string(&lf.config.parallelism)
                        .expect("ParallelismMode is json-serializable"),
                    gate_after_each: lf.config.gate_after_each,
                    items_json: lf.items.into_iter().map(toml_value_to_json).collect(),
                    current_index: lf.current_index,
                    attempts_remaining: lf.attempts_remaining,
                    return_to: lf.return_to.to_string(),
                    traversal_counts: lf
                        .traversal_counts
                        .into_iter()
                        .map(|(k, v)| (k.to_string(), v))
                        .collect(),
                })
            }
            crate::engine::frames::Frame::Subgraph(sf) => {
                Self::Subgraph(SerializableSubgraphFrame {
                    outer_node: sf.outer_node.to_string(),
                    inner_subgraph: sf.inner_subgraph.to_string(),
                    bound_inputs: sf
                        .bound_inputs
                        .into_iter()
                        .map(|i| SerializableSubgraphInput {
                            inner_var: i.inner_var.0,
                            value: i.value,
                        })
                        .collect(),
                    return_to: sf.return_to.to_string(),
                })
            }
        }
    }
}

fn toml_value_to_json(v: toml::Value) -> serde_json::Value {
    serde_json::to_value(&v).unwrap_or(serde_json::Value::Null)
}

impl EngineSnapshot {
    /// Current schema version. Bump on any breaking layout change.
    /// Version 2 (M6) — adds `frames` (Loop/Subgraph nesting) and
    /// `root_traversal_counts` (max_traversals enforcement outside loops).
    pub const SCHEMA_VERSION: u32 = 2;

    /// Create a new snapshot for the given cursor and sequence numbers.
    /// Frames and traversal counts default to empty.
    #[must_use]
    pub fn new(cursor: &Cursor, at_seq: u64, stage_boundary_seq: u64) -> Self {
        Self {
            schema_version: Self::SCHEMA_VERSION,
            cursor: SerializableCursor::from(cursor),
            frames: Vec::new(),
            root_traversal_counts: HashMap::new(),
            at_seq,
            stage_boundary_seq,
            pending_human_input: None,
        }
    }

    /// Deserialise from a JSON blob. Reads schema_version first and routes
    /// to either v1 back-compat path or direct v2 deserialisation.
    pub fn deserialize(blob: &[u8]) -> Result<Self, SnapshotError> {
        let value: serde_json::Value = serde_json::from_slice(blob)
            .map_err(|e| SnapshotError::InvalidJson(e.to_string()))?;
        let version = value.get("schema_version").and_then(|v| v.as_u64());

        match version {
            Some(1) => {
                #[derive(Deserialize)]
                struct V1 {
                    cursor: SerializableCursor,
                    at_seq: u64,
                    stage_boundary_seq: u64,
                    #[serde(default)]
                    pending_human_input: Option<PendingHumanInputSnapshot>,
                }
                let v1: V1 = serde_json::from_value(value)
                    .map_err(|e| SnapshotError::InvalidJson(format!("v1 parse: {e}")))?;
                Ok(Self {
                    schema_version: Self::SCHEMA_VERSION, // upgrade tag
                    cursor: v1.cursor,
                    frames: Vec::new(),
                    root_traversal_counts: HashMap::new(),
                    at_seq: v1.at_seq,
                    stage_boundary_seq: v1.stage_boundary_seq,
                    pending_human_input: v1.pending_human_input,
                })
            }
            Some(2) => serde_json::from_value(value)
                .map_err(|e| SnapshotError::InvalidJson(format!("v2 parse: {e}"))),
            other => Err(SnapshotError::UnsupportedSchema(other)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::frames::{Frame, LoopFrame};
    use std::collections::HashMap;
    use surge_core::keys::SubgraphKey;
    use surge_core::loop_config::{
        ExitCondition, FailurePolicy, IterableSource, LoopConfig, ParallelismMode,
    };

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

    #[test]
    fn v2_with_empty_frames_roundtrips() {
        let cursor = Cursor {
            node: NodeKey::try_from("plan_1").unwrap(),
            attempt: 1,
        };
        let snap = EngineSnapshot::new(&cursor, 42, 41);
        assert_eq!(snap.schema_version, 2);
        assert!(snap.frames.is_empty());
        assert!(snap.root_traversal_counts.is_empty());

        let json = serde_json::to_vec(&snap).unwrap();
        let parsed: EngineSnapshot = serde_json::from_slice(&json).unwrap();
        assert_eq!(snap, parsed);
    }

    #[test]
    fn v2_with_loop_frame_roundtrips() {
        let cursor = Cursor {
            node: NodeKey::try_from("inner_step").unwrap(),
            attempt: 1,
        };
        let mut snap = EngineSnapshot::new(&cursor, 100, 90);

        let loop_frame = LoopFrame {
            loop_node: NodeKey::try_from("loop_1").unwrap(),
            config: LoopConfig {
                iterates_over: IterableSource::Static(vec![toml::Value::Integer(1)]),
                body: SubgraphKey::try_from("body").unwrap(),
                iteration_var_name: "item".into(),
                exit_condition: ExitCondition::AllItems,
                on_iteration_failure: FailurePolicy::Abort,
                parallelism: ParallelismMode::Sequential,
                gate_after_each: false,
            },
            items: vec![toml::Value::Integer(1)],
            current_index: 0,
            attempts_remaining: 0,
            return_to: NodeKey::try_from("after_loop").unwrap(),
            traversal_counts: HashMap::new(),
        };
        snap.frames = vec![SerializableFrame::from(Frame::Loop(loop_frame))];

        let json = serde_json::to_vec(&snap).unwrap();
        let parsed: EngineSnapshot = serde_json::from_slice(&json).unwrap();
        assert_eq!(snap, parsed);
    }

    #[test]
    fn v1_blob_deserialises_via_back_compat_reader() {
        // Hand-crafted v1 blob (no `frames` field, schema_version = 1).
        let v1_json = r#"{
            "schema_version": 1,
            "cursor": { "node": "plan_1", "attempt": 1 },
            "at_seq": 42,
            "stage_boundary_seq": 41,
            "pending_human_input": null
        }"#;
        let snap = EngineSnapshot::deserialize(v1_json.as_bytes()).expect("v1 reader works");
        assert_eq!(snap.schema_version, 2);
        assert!(snap.frames.is_empty());
        assert!(snap.root_traversal_counts.is_empty());
        assert_eq!(snap.cursor.node, "plan_1");
    }
}
