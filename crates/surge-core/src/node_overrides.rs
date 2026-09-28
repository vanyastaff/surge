//! Operator edits to a plan's steps, made while approving it.
//!
//! The approval gate for a generated flow can carry `node_overrides` next to
//! its outcome. They are part of the durable decision (the gate's resolved
//! response in the event log) and are applied deterministically to the
//! approved graph before the implementation run starts, so what runs is
//! exactly what the operator saw and changed.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::graph::Graph;
use crate::node::NodeConfig;

/// Edits for one agent step.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeOverride {
    /// Run this step on another provider (registry id, e.g. `codex-acp`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
}

/// Edits keyed by node id (top-level or inside a loop/sub-flow body).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NodeOverrides(pub BTreeMap<String, NodeOverride>);

/// Why a set of overrides cannot be applied to a graph.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NodeOverrideError {
    #[error("plan edit targets unknown step '{0}'")]
    UnknownNode(String),
    #[error("plan edit targets step '{0}', which is not an agent step")]
    NotAnAgent(String),
}

impl NodeOverrides {
    /// Read `node_overrides` from a gate's resolved response, if present.
    ///
    /// # Errors
    /// Returns the deserialization error when the field exists but is
    /// malformed — a malformed edit must not be dropped silently.
    pub fn from_gate_response(response: &serde_json::Value) -> Result<Self, serde_json::Error> {
        match response.get("node_overrides") {
            None | Some(serde_json::Value::Null) => Ok(Self::default()),
            Some(value) => serde_json::from_value(value.clone()),
        }
    }

    /// True when there is nothing to apply.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.values().all(|o| o.agent_id.is_none())
    }

    /// Apply to `graph` in place.
    ///
    /// # Errors
    /// Fails without partial application when any key is unknown or names a
    /// non-agent step.
    pub fn apply(&self, graph: &mut Graph) -> Result<(), NodeOverrideError> {
        for key in self.0.keys() {
            let node = graph
                .nodes
                .iter()
                .chain(graph.subgraphs.values().flat_map(|s| s.nodes.iter()))
                .find(|(k, _)| k.as_str() == key)
                .map(|(_, n)| n)
                .ok_or_else(|| NodeOverrideError::UnknownNode(key.clone()))?;
            if !matches!(node.config, NodeConfig::Agent(_)) {
                return Err(NodeOverrideError::NotAnAgent(key.clone()));
            }
        }
        let nodes = graph.nodes.iter_mut().chain(
            graph
                .subgraphs
                .values_mut()
                .flat_map(|s| s.nodes.iter_mut()),
        );
        for (key, node) in nodes {
            let (Some(edit), NodeConfig::Agent(agent)) =
                (self.0.get(key.as_str()), &mut node.config)
            else {
                continue;
            };
            if let Some(agent_id) = edit
                .agent_id
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                let mut runtime = toml::map::Map::new();
                runtime.insert("agent_id".into(), toml::Value::String(agent_id.to_string()));
                agent
                    .custom_fields
                    .insert("runtime".into(), toml::Value::Table(runtime));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn graph() -> Graph {
        crate::BundledFlows::by_name_latest("linear-3")
            .expect("bundled")
            .graph
    }

    fn agent_key(graph: &Graph) -> String {
        graph
            .nodes
            .iter()
            .find(|(_, n)| matches!(n.config, NodeConfig::Agent(_)))
            .map(|(k, _)| k.as_str().to_string())
            .expect("an agent step")
    }

    #[test]
    fn provider_edit_lands_on_the_step() {
        let mut g = graph();
        let key = agent_key(&g);
        let response = serde_json::json!({
            "outcome": "approve",
            "node_overrides": { key.clone(): { "agent_id": "codex-acp" } }
        });
        let edits = NodeOverrides::from_gate_response(&response).expect("parses");
        edits.apply(&mut g).expect("applies");
        let NodeConfig::Agent(agent) = &g
            .nodes
            .iter()
            .find(|(k, _)| k.as_str() == key)
            .unwrap()
            .1
            .config
        else {
            unreachable!()
        };
        assert_eq!(agent.runtime_override(), Some("codex-acp"));
    }

    #[test]
    fn unknown_or_non_agent_targets_are_refused_whole() {
        let mut g = graph();
        let before = g.clone();
        let bad = NodeOverrides(BTreeMap::from([(
            "no_such_step".into(),
            NodeOverride {
                agent_id: Some("codex-acp".into()),
            },
        )]));
        assert!(matches!(
            bad.apply(&mut g),
            Err(NodeOverrideError::UnknownNode(_))
        ));
        assert_eq!(g, before, "no partial application");
        let terminal = g
            .nodes
            .iter()
            .find(|(_, n)| matches!(n.config, NodeConfig::Terminal(_)))
            .map(|(k, _)| k.as_str().to_string())
            .unwrap();
        let bad = NodeOverrides(BTreeMap::from([(
            terminal,
            NodeOverride {
                agent_id: Some("codex-acp".into()),
            },
        )]));
        assert!(matches!(
            bad.apply(&mut g),
            Err(NodeOverrideError::NotAnAgent(_))
        ));
    }

    #[test]
    fn absent_or_malformed_edits_are_distinguished() {
        let none = NodeOverrides::from_gate_response(&serde_json::json!({"outcome": "approve"}));
        assert!(none.expect("absent is fine").is_empty());
        let bad = NodeOverrides::from_gate_response(&serde_json::json!({"node_overrides": 3}));
        assert!(bad.is_err(), "malformed edits must surface");
    }
}
