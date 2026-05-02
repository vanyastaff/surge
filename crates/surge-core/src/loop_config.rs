//! Loop node configuration.

use crate::keys::{NodeKey, OutcomeKey, SubgraphKey};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LoopConfig {
    pub iterates_over: IterableSource,
    /// Subgraph to execute per iteration. References `Graph::subgraphs[body]`.
    pub body: SubgraphKey,
    pub iteration_var_name: String,
    pub exit_condition: ExitCondition,
    #[serde(default)]
    pub on_iteration_failure: FailurePolicy,
    #[serde(default)]
    pub parallelism: ParallelismMode,
    #[serde(default)]
    pub gate_after_each: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum IterableSource {
    Artifact { node: NodeKey, name: String, jsonpath: String },
    Static(Vec<toml::Value>),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ExitCondition {
    AllItems,
    UntilOutcome { from_node: NodeKey, outcome: OutcomeKey },
    MaxIterations { n: u32 },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FailurePolicy {
    #[default]
    Abort,
    Skip,
    Retry { max: u32 },
    Replan,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParallelismMode {
    #[default]
    Sequential,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loop_with_artifact_iterator_roundtrips() {
        let cfg = LoopConfig {
            iterates_over: IterableSource::Artifact {
                node: NodeKey::try_from("roadmap_1").unwrap(),
                name: "roadmap.md".into(),
                jsonpath: "$.milestones[*]".into(),
            },
            body: SubgraphKey::try_from("milestone_body").unwrap(),
            iteration_var_name: "milestone".into(),
            exit_condition: ExitCondition::AllItems,
            on_iteration_failure: FailurePolicy::Retry { max: 2 },
            parallelism: ParallelismMode::Sequential,
            gate_after_each: false,
        };
        let toml_s = toml::to_string(&cfg).unwrap();
        let parsed: LoopConfig = toml::from_str(&toml_s).unwrap();
        assert_eq!(cfg, parsed);
    }

    #[test]
    fn default_failure_policy_is_abort() {
        assert!(matches!(FailurePolicy::default(), FailurePolicy::Abort));
    }
}
