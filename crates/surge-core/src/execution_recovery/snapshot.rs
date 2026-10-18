//! Shared snapshot wire DTOs; decoding grants no executable frame authority.
use crate::{keys::NodeKey, run_state::Cursor};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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
    /// The blob has a `schema_version` this engine doesn't support.
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

/// Serde-friendly mirror of the engine frame stack.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SerializableFrame {
    /// Serialized form of a `Frame::Loop` entry.
    Loop(SerializableLoopFrame),
    /// Serialized form of a `Frame::Subgraph` entry.
    Subgraph(SerializableSubgraphFrame),
}

/// Serializable form of a the engine loop frame.
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

/// Serializable form of a the engine subgraph frame.
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
