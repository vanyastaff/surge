//! Top-level pipeline graph.

use crate::archetype::ArchetypeMetadata;
use crate::edge::Edge;
use crate::keys::{NodeKey, SubgraphKey, TemplateKey};
use crate::node::Node;
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
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

impl Graph {
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
            when_to_use: None,
            autonomy: None,
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
    /// One-line guidance for the flow selector: what kind of task this
    /// template fits ("a bug with a reproduction", "a broad refactor across
    /// modules"). The classifier reads this instead of the graph shape.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when_to_use: Option<String>,
    /// Where the operator wanted `human_gate` nodes placed when this
    /// template is used for a task. `None` means the template's own gates
    /// stand as authored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub autonomy: Option<AutonomyLevel>,
}

/// How much human supervision a task flow asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AutonomyLevel {
    /// Run the whole task without gates unless one is already authored in.
    Auto,
    /// Gate at milestone boundaries only.
    MilestoneGates,
    /// Gate after every task.
    TaskGates,
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
                when_to_use: None,
                autonomy: None,
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
