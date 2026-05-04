//! Graph node.

use crate::agent_config::AgentConfig;
use crate::branch_config::BranchConfig;
use crate::edge::EdgeKind;
use crate::human_gate_config::HumanGateConfig;
use crate::keys::{NodeKey, OutcomeKey};
use crate::loop_config::LoopConfig;
use crate::notify_config::NotifyConfig;
use crate::subgraph_config::SubgraphConfig;
use crate::terminal_config::TerminalConfig;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Node {
    pub id: NodeKey,
    #[serde(default)]
    pub position: Position,
    #[serde(default)]
    pub declared_outcomes: Vec<OutcomeDecl>,
    pub config: NodeConfig,
}

impl Node {
    #[must_use]
    pub fn kind(&self) -> NodeKind {
        self.config.kind()
    }
}

/// Closed enum of supported node types. Adding a variant requires editing core.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    Agent,
    HumanGate,
    Branch,
    Terminal,
    Notify,
    Loop,
    Subgraph,
}

/// Node configuration — internally tagged by `node_kind` field in TOML.
///
/// Uses `node_kind` rather than `kind` to avoid a TOML field-name collision
/// with `TerminalConfig::kind` (which is itself an internally-tagged enum
/// serialised into the same table). The semantic meaning is identical.
// AgentConfig is significantly larger than other variants; that is expected
// for the richest node type in the graph model. Boxing is intentionally
// deferred: callers construct NodeConfig directly in TOML round-trips and
// property tests, so the ergonomic cost of Box outweighs the stack savings.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "node_kind", rename_all = "snake_case")]
pub enum NodeConfig {
    Agent(AgentConfig),
    HumanGate(HumanGateConfig),
    Branch(BranchConfig),
    Terminal(TerminalConfig),
    Notify(NotifyConfig),
    Loop(LoopConfig),
    Subgraph(SubgraphConfig),
}

impl NodeConfig {
    #[must_use]
    pub fn kind(&self) -> NodeKind {
        match self {
            Self::Agent(_) => NodeKind::Agent,
            Self::HumanGate(_) => NodeKind::HumanGate,
            Self::Branch(_) => NodeKind::Branch,
            Self::Terminal(_) => NodeKind::Terminal,
            Self::Notify(_) => NodeKind::Notify,
            Self::Loop(_) => NodeKind::Loop,
            Self::Subgraph(_) => NodeKind::Subgraph,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Position {
    pub x: f32,
    pub y: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OutcomeDecl {
    pub id: OutcomeKey,
    pub description: String,
    pub edge_kind_hint: EdgeKind,
    #[serde(default)]
    pub is_terminal: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::ProfileKey;
    use crate::terminal_config::TerminalKind;

    #[test]
    fn agent_node_kind_derives_from_config() {
        let cfg = NodeConfig::Agent(AgentConfig {
            profile: ProfileKey::try_from("implementer@1.0").unwrap(),
            prompt_overrides: None,
            tool_overrides: None,
            sandbox_override: None,
            approvals_override: None,
            bindings: Vec::new(),
            rules_overrides: None,
            limits: Default::default(),
            hooks: Vec::new(),
            custom_fields: Default::default(),
        });
        assert_eq!(cfg.kind(), NodeKind::Agent);
    }

    #[test]
    fn terminal_node_roundtrip_via_toml() {
        let n = Node {
            id: NodeKey::try_from("end").unwrap(),
            position: Position { x: 100.0, y: 200.0 },
            declared_outcomes: vec![],
            config: NodeConfig::Terminal(TerminalConfig {
                kind: TerminalKind::Success,
                message: Some("Done".into()),
            }),
        };
        let toml_s = toml::to_string(&n).unwrap();
        let parsed: Node = toml::from_str(&toml_s).unwrap();
        assert_eq!(n, parsed);
    }

    #[test]
    fn outcome_decl_roundtrip() {
        let o = OutcomeDecl {
            id: OutcomeKey::try_from("done").unwrap(),
            description: "Success path".into(),
            edge_kind_hint: EdgeKind::Forward,
            is_terminal: false,
        };
        let toml_s = toml::to_string(&o).unwrap();
        let parsed: OutcomeDecl = toml::from_str(&toml_s).unwrap();
        assert_eq!(o, parsed);
    }
}
