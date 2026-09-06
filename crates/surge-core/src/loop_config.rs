//! Loop node configuration.
//!
//! Also hosts one unrelated `surge.toml`-level threshold config —
//! [`ToolCallLoopGuardConfig`] — consumed by `surge_orchestrator::guard`. It
//! guards against a *tool call* repeating inside any node's agent stage,
//! which is a different concept from the `Loop` flow-graph node
//! ([`LoopConfig`]) this file otherwise defines, but the name collision
//! ("loop") is the reason it lives here rather than its own module. The
//! output-spill counterpart, [`crate::spill_config::OutputSpillConfig`],
//! moved to its own module — spilling large tool output has nothing to do
//! with "loop" in either sense.

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
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum IterableSource {
    Artifact {
        node: NodeKey,
        name: String,
        jsonpath: String,
    },
    LoopItem {
        var: String,
        jsonpath: String,
    },
    Static(Vec<toml::Value>),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ExitCondition {
    AllItems,
    UntilOutcome {
        from_node: NodeKey,
        outcome: OutcomeKey,
    },
    MaxIterations {
        n: u32,
    },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FailurePolicy {
    #[default]
    Abort,
    Skip,
    Retry {
        max: u32,
    },
    Replan,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParallelismMode {
    #[default]
    Sequential,
}

/// Maximum number of items in `IterableSource::Static`. Larger static
/// lists are rejected at TOML load (graph validation) to bound memory
/// in the engine's `LoopFrame::items`. The engine enforces a parallel
/// cap on resolved artifact-derived iterables (`MAX_LOOP_ITEMS_RESOLVED`,
/// also 1000, enforced engine-side at frame-push time).
pub const MAX_LOOP_ITEMS_STATIC: usize = 1000;

/// Engine-level guard against a node's agent stage repeating the identical
/// tool call, or running past a wall-clock budget, instead of making
/// progress. The engine counts and decides (`surge_orchestrator::guard`);
/// the agent never self-polices its own looping. `surge.toml` key:
/// `tool_call_loop_guard`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolCallLoopGuardConfig {
    /// How many consecutive identical tool calls (same tool name, same
    /// arguments) are tolerated before the engine escalates instead of
    /// dispatching the next one. `3` means the 4th consecutive identical
    /// call is refused.
    #[serde(default = "default_max_repeat_tool_calls")]
    pub max_repeat_tool_calls: u32,
    /// Wall-clock budget, in seconds, for one node's agent stage before the
    /// engine escalates regardless of tool-call pattern.
    #[serde(default = "default_node_wall_clock_limit_secs")]
    pub node_wall_clock_limit_secs: u64,
}

impl Default for ToolCallLoopGuardConfig {
    fn default() -> Self {
        Self {
            max_repeat_tool_calls: default_max_repeat_tool_calls(),
            node_wall_clock_limit_secs: default_node_wall_clock_limit_secs(),
        }
    }
}

/// Conservative default: three consecutive identical calls are ordinary
/// (an agent re-reading a file it just wrote, retrying a flaky command);
/// a fourth is a strong loop signal.
fn default_max_repeat_tool_calls() -> u32 {
    3
}

/// Conservative default: one hour. Long enough that a legitimately
/// long-running node (a large build, an extensive review) is not
/// mistaken for a stall, short enough to eventually surface a genuine one.
fn default_node_wall_clock_limit_secs() -> u64 {
    3600
}

#[cfg(test)]
mod guard_config_tests {
    use super::*;

    #[test]
    fn tool_call_loop_guard_config_toml_roundtrip() {
        let cfg = ToolCallLoopGuardConfig {
            max_repeat_tool_calls: 5,
            node_wall_clock_limit_secs: 120,
        };
        let toml_s = toml::to_string(&cfg).unwrap();
        let parsed: ToolCallLoopGuardConfig = toml::from_str(&toml_s).unwrap();
        assert_eq!(cfg, parsed);
    }

    #[test]
    fn tool_call_loop_guard_config_defaults_are_conservative() {
        let cfg = ToolCallLoopGuardConfig::default();
        assert_eq!(cfg.max_repeat_tool_calls, 3);
        assert_eq!(cfg.node_wall_clock_limit_secs, 3600);
    }

    #[test]
    fn tool_call_loop_guard_config_absent_fields_default_on_parse() {
        // Backward-compat guard: a `surge.toml` predating this key parses
        // as an empty table and must decode with the conservative defaults.
        let parsed: ToolCallLoopGuardConfig = toml::from_str("").unwrap();
        assert_eq!(parsed, ToolCallLoopGuardConfig::default());
    }
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
    fn loop_with_loop_item_iterator_roundtrips() {
        let cfg = LoopConfig {
            iterates_over: IterableSource::LoopItem {
                var: "milestone".into(),
                jsonpath: "tasks".into(),
            },
            body: SubgraphKey::try_from("task_body").unwrap(),
            iteration_var_name: "task".into(),
            exit_condition: ExitCondition::AllItems,
            on_iteration_failure: FailurePolicy::Abort,
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
