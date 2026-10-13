//! Top-level pipeline graph.

use crate::archetype::ArchetypeMetadata;
use crate::edge::Edge;
use crate::keys::{NodeKey, SubgraphKey, TemplateKey};
use crate::node::Node;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const SCHEMA_VERSION: u32 = 1;

/// Top-level pipeline graph. One per `flow.toml`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Graph {
    pub schema_version: u32,
    pub metadata: GraphMetadata,
    pub start: NodeKey,
    pub nodes: BTreeMap<NodeKey, Node>,
    pub edges: Vec<Edge>,
    /// Library of named subgraphs. `Loop.body` and `Subgraph.inner` reference
    /// entries here. Always lives at the root.
    #[serde(default)]
    pub subgraphs: BTreeMap<SubgraphKey, Subgraph>,
}

/// Computing a graph's identity failed (its JSON serialization failed).
#[derive(Debug, thiserror::Error)]
#[error("graph identity serialization failed: {0}")]
pub struct GraphIdentityError(#[from] pub serde_json::Error);

impl Graph {
    /// The graph's canonical identity (#98): the content hash of
    /// `serde_json::to_vec(self)` in **struct field order**. The one graph
    /// hash every surface records and compares: `PipelineMaterialized`
    /// (engine startup and the bootstrap stage), daemon continuation pins and
    /// their recovery recomputation, and roadmap/flow amendment revisions
    /// (`previous_graph_hash`).
    ///
    /// Byte-identical to every hash persisted before this method existed,
    /// which is why the form is never "sorted canonical JSON". Guards that keep
    /// it stable (pinned by tests):
    /// - every map reachable from `Graph` is a `BTreeMap`, and embedded
    ///   `toml::Value` tables are BTreeMaps (`toml` is built without
    ///   `indexmap`), so map order never depends on insertion order;
    /// - no `serde_json::Value` or `HashMap` may live inside `Graph`:
    ///   `serde_json/preserve_order` is enabled workspace-wide;
    /// - a new `Graph` field must `skip_serializing_if` its default, so
    ///   existing graphs keep their identity.
    ///
    /// # Errors
    /// [`GraphIdentityError`] if the graph cannot be serialized.
    pub fn identity(&self) -> Result<crate::content_hash::ContentHash, GraphIdentityError> {
        Ok(crate::content_hash::ContentHash::compute(
            &serde_json::to_vec(self)?,
        ))
    }

    /// Find a node by key at the top level **or inside any subgraph**.
    ///
    /// Node keys are unique across the graph (top-level + subgraph bodies), so a
    /// single lookup is unambiguous. Prefer this over open-coding the
    /// top-level-plus-subgraphs walk: a top-level-only `nodes.get()` silently
    /// misses nodes declared in a loop/subgraph body (the class of bug behind
    /// the verification-authority and resolve-gate-options fixes).
    #[must_use]
    pub fn find_node(&self, key: &NodeKey) -> Option<&Node> {
        self.nodes
            .get(key)
            .or_else(|| self.subgraphs.values().find_map(|sg| sg.nodes.get(key)))
    }
}

/// A named, reusable inner graph. Lighter than `Graph` — no metadata,
/// no nested subgraphs library.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Subgraph {
    pub start: NodeKey,
    pub nodes: BTreeMap<NodeKey, Node>,
    pub edges: Vec<Edge>,
}

impl GraphMetadata {
    /// Construct a minimal metadata block with `name` and `created_at`.
    /// All other fields default to `None`.
    #[must_use]
    pub fn new(name: impl Into<String>, created_at: DateTime<Utc>) -> Self {
        Self {
            name: name.into(),
            description: None,
            template_origin: None,
            created_at,
            author: None,
            archetype: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GraphMetadata {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub template_origin: Option<TemplateKey>,
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub author: Option<String>,
    /// Optional archetype tag attached by Flow Generator (or hand-authored).
    /// `None` for graphs that pre-date the bootstrap milestone.
    #[serde(default)]
    pub archetype: Option<ArchetypeMetadata>,
}

#[cfg(test)]
mod identity_tests {
    use super::*;

    /// Captured from the form every persisted `PipelineMaterialized` and
    /// continuation pin uses; a serializer or field change that alters it
    /// would orphan every recorded hash.
    const LINEAR_3_IDENTITY: &str =
        "11b2007d0975100b8d57f1628e10221170ed6073a3cbda02cd21ce04914c72b0";

    #[test]
    fn identity_is_todays_persisted_json_digest() {
        let graph = crate::BundledFlows::by_name_latest("linear-3")
            .unwrap()
            .graph;
        let persisted =
            crate::content_hash::ContentHash::compute(&serde_json::to_vec(&graph).unwrap());
        assert_eq!(graph.identity().unwrap(), persisted);
        assert_eq!(graph.identity().unwrap().to_hex(), LINEAR_3_IDENTITY);
    }

    #[test]
    fn identity_survives_a_toml_round_trip_and_tracks_edits() {
        let graph = crate::BundledFlows::by_name_latest("linear-3")
            .unwrap()
            .graph;
        let round_tripped: Graph = toml::from_str(&toml::to_string(&graph).unwrap()).unwrap();
        assert_eq!(round_tripped.identity().unwrap(), graph.identity().unwrap());
        let mut edited = graph.clone();
        edited.metadata.name.push_str("-edited");
        assert_ne!(edited.identity().unwrap(), graph.identity().unwrap());
    }

    #[test]
    fn toml_table_insertion_order_does_not_change_identity() {
        fn with_extra(order: &[(&str, i64)]) -> Graph {
            let mut graph = crate::BundledFlows::by_name_latest("linear-3")
                .unwrap()
                .graph;
            let agent = graph
                .nodes
                .values_mut()
                .find_map(|node| match &mut node.config {
                    crate::node::NodeConfig::Agent(agent) => Some(agent),
                    _ => None,
                })
                .unwrap();
            let mut table = toml::map::Map::new();
            for (key, value) in order {
                table.insert((*key).into(), toml::Value::Integer(*value));
            }
            agent
                .custom_fields
                .insert("extra".into(), toml::Value::Table(table));
            graph
        }
        let forward = with_extra(&[("alpha", 1), ("zeta", 2)]);
        let backward = with_extra(&[("zeta", 2), ("alpha", 1)]);
        assert_eq!(forward.identity().unwrap(), backward.identity().unwrap());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_graph_compiles_and_serializes() {
        let g = Graph {
            schema_version: SCHEMA_VERSION,
            metadata: GraphMetadata {
                name: "empty".into(),
                description: None,
                template_origin: None,
                created_at: chrono::Utc::now(),
                author: None,
                archetype: None,
            },
            start: NodeKey::try_from("placeholder").unwrap(),
            nodes: BTreeMap::new(),
            edges: Vec::new(),
            subgraphs: BTreeMap::new(),
        };
        let _toml_s = toml::to_string(&g).unwrap();
    }

    #[test]
    fn schema_version_constant_is_one() {
        assert_eq!(SCHEMA_VERSION, 1);
    }
}
