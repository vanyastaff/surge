//! Graph edge types.

use crate::keys::{EdgeKey, NodeKey, OutcomeKey};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Edge {
    pub id: EdgeKey,
    pub from: PortRef,
    pub to: NodeKey,
    pub kind: EdgeKind,
    #[serde(default)]
    pub policy: EdgePolicy,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct PortRef {
    pub node: NodeKey,
    pub outcome: OutcomeKey,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    /// Standard forward routing — every edge in M5/M6 graphs.
    #[default]
    Forward,
    /// Loop back to a previously visited node (e.g. a HumanGate `edit`
    /// outcome routing back to its preceding Agent node so the agent can
    /// re-resolve bindings against fresh feedback). Routing emits
    /// `EdgeTraversed { kind: Backtrack }` and increments
    /// `RunMemory.node_visits[target]`. Bootstrap-mode HumanGates rely on
    /// this to power the operator-driven edit loop.
    Backtrack,
    /// Reserved for max-traversal escalation re-routes — runtime semantics
    /// land in a follow-up milestone.
    Escalate,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct EdgePolicy {
    #[serde(default)]
    pub max_traversals: Option<u32>,
    #[serde(default)]
    pub on_max_exceeded: ExceededAction,
    #[serde(default)]
    pub label: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExceededAction {
    #[default]
    Escalate,
    Fail,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edge_toml_roundtrip() {
        let e = Edge {
            id: EdgeKey::try_from("e_spec_to_plan").unwrap(),
            from: PortRef {
                node: NodeKey::try_from("spec_1").unwrap(),
                outcome: OutcomeKey::try_from("done").unwrap(),
            },
            to: NodeKey::try_from("plan_1").unwrap(),
            kind: EdgeKind::Forward,
            policy: EdgePolicy::default(),
        };
        let toml_s = toml::to_string(&e).unwrap();
        let parsed: Edge = toml::from_str(&toml_s).unwrap();
        assert_eq!(e, parsed);
    }

    #[test]
    fn backtrack_default_policy_escalates() {
        let p = EdgePolicy::default();
        assert_eq!(p.on_max_exceeded, ExceededAction::Escalate);
        assert!(p.max_traversals.is_none());
    }

    #[test]
    fn edge_kind_serializes_snake_case() {
        let json = serde_json::json!(EdgeKind::Backtrack);
        assert_eq!(json, "backtrack");
    }
}
