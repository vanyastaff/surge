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
