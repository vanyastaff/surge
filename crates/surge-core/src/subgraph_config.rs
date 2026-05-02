//! Subgraph node configuration.

use crate::agent_config::{ArtifactSource, Binding, TemplateVar};
use crate::keys::{OutcomeKey, SubgraphKey};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SubgraphConfig {
    /// Inner subgraph to execute. References `Graph::subgraphs[inner]`.
    pub inner: SubgraphKey,
    pub inputs: Vec<SubgraphInput>,
    pub outputs: Vec<SubgraphOutput>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SubgraphInput {
    pub outer_binding: Binding,
    pub inner_var: TemplateVar,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SubgraphOutput {
    pub inner_artifact: ArtifactSource,
    pub outer_outcome: OutcomeKey,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::NodeKey;

    #[test]
    fn subgraph_config_roundtrips() {
        let cfg = SubgraphConfig {
            inner: SubgraphKey::try_from("review_block").unwrap(),
            inputs: vec![SubgraphInput {
                outer_binding: Binding {
                    source: ArtifactSource::NodeOutput {
                        node: NodeKey::try_from("plan_1").unwrap(),
                        artifact: "plan.md".into(),
                    },
                    target: TemplateVar("plan".into()),
                },
                inner_var: TemplateVar("plan".into()),
            }],
            outputs: vec![SubgraphOutput {
                inner_artifact: ArtifactSource::NodeOutput {
                    node: NodeKey::try_from("review_inner").unwrap(),
                    artifact: "review.md".into(),
                },
                outer_outcome: OutcomeKey::try_from("done").unwrap(),
            }],
        };
        let toml_s = toml::to_string(&cfg).unwrap();
        let parsed: SubgraphConfig = toml::from_str(&toml_s).unwrap();
        assert_eq!(cfg, parsed);
    }
}
