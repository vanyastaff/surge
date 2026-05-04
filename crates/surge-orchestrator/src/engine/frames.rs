//! Frame stack — nested execution context for `Loop` and `Subgraph` nodes.
//!
//! The engine remains single-threaded per run (revision §03-engine §Concurrency
//! model). The cursor names "the one node we are about to execute next"; the
//! frame stack records "what we will do when the cursor reaches a terminal
//! node inside an inner graph". See spec §2.2.

use std::collections::HashMap;
use surge_core::agent_config::TemplateVar;
use surge_core::keys::{EdgeKey, NodeKey, SubgraphKey};
use surge_core::loop_config::{FailurePolicy, LoopConfig};

/// Maximum number of items in a resolved `LoopConfig::iterates_over::Artifact`
/// iterable. See spec §2.4. Mirrors `surge_core::loop_config::MAX_LOOP_ITEMS_STATIC`.
pub const MAX_LOOP_ITEMS_RESOLVED: usize = 1000;

/// Single entry on the per-run frame stack.
#[derive(Debug, Clone, PartialEq)]
pub enum Frame {
    /// Pushed on entering a `NodeKind::Loop`.
    Loop(LoopFrame),
    /// Pushed on entering a `NodeKind::Subgraph`.
    Subgraph(SubgraphFrame),
}

/// Loop iteration state.
#[derive(Debug, Clone, PartialEq)]
pub struct LoopFrame {
    /// `NodeKey` of the outer Loop node.
    pub loop_node: NodeKey,
    /// Loop configuration (body subgraph reference, exit condition, …).
    pub config: LoopConfig,
    /// Resolved iteration items. Length is bounded by `MAX_LOOP_ITEMS_RESOLVED`.
    pub items: Vec<toml::Value>,
    /// Index of the current iteration (0-based).
    pub current_index: u32,
    /// Remaining retries for the current iteration (only used when
    /// `config.on_iteration_failure` is `FailurePolicy::Retry`).
    pub attempts_remaining: u32,
    /// Outer-graph node to advance to when the loop exits.
    pub return_to: NodeKey,
    /// Per-edge traversal counter for body edges, for `EdgePolicy::max_traversals`.
    pub traversal_counts: HashMap<EdgeKey, u32>,
}

/// Subgraph execution state.
#[derive(Debug, Clone, PartialEq)]
pub struct SubgraphFrame {
    /// `NodeKey` of the outer Subgraph node.
    pub outer_node: NodeKey,
    /// `SubgraphKey` referencing the inner subgraph in `Graph::subgraphs`.
    pub inner_subgraph: SubgraphKey,
    /// Resolved input bindings, mapping inner template vars → values.
    pub bound_inputs: Vec<ResolvedSubgraphInput>,
    /// Outer-graph node to advance to when the subgraph exits.
    pub return_to: NodeKey,
}

/// One resolved subgraph input: the inner template variable bound to a value.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedSubgraphInput {
    /// Inner template variable name (e.g. `{{plan}}`).
    pub inner_var: TemplateVar,
    /// Resolved value as a serde JSON value (uniform across artifact /
    /// static / inline sources).
    pub value: serde_json::Value,
}

/// Initial-attempt counter for `FailurePolicy::Retry`.
#[must_use]
pub fn initial_attempts_remaining(policy: &FailurePolicy) -> u32 {
    match policy {
        FailurePolicy::Retry { max } => *max,
        _ => 0,
    }
}

/// Active loop frame at the top of the stack, if any.
#[must_use]
pub fn top_loop_mut(frames: &mut [Frame]) -> Option<&mut LoopFrame> {
    match frames.last_mut() {
        Some(Frame::Loop(lf)) => Some(lf),
        _ => None,
    }
}

/// Active subgraph frame at the top of the stack, if any.
#[must_use]
pub fn top_subgraph(frames: &[Frame]) -> Option<&SubgraphFrame> {
    match frames.last() {
        Some(Frame::Subgraph(sf)) => Some(sf),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use surge_core::loop_config::{ExitCondition, IterableSource, ParallelismMode};

    fn empty_loop_config() -> LoopConfig {
        LoopConfig {
            iterates_over: IterableSource::Static(vec![]),
            body: SubgraphKey::try_from("body").unwrap(),
            iteration_var_name: "item".into(),
            exit_condition: ExitCondition::AllItems,
            on_iteration_failure: FailurePolicy::Abort,
            parallelism: ParallelismMode::Sequential,
            gate_after_each: false,
        }
    }

    #[test]
    fn initial_attempts_zero_for_abort() {
        assert_eq!(initial_attempts_remaining(&FailurePolicy::Abort), 0);
    }

    #[test]
    fn initial_attempts_returns_max_for_retry() {
        assert_eq!(initial_attempts_remaining(&FailurePolicy::Retry { max: 3 }), 3);
    }

    #[test]
    fn top_loop_mut_returns_top_frame() {
        let lf = LoopFrame {
            loop_node: NodeKey::try_from("loop_1").unwrap(),
            config: empty_loop_config(),
            items: vec![],
            current_index: 0,
            attempts_remaining: 0,
            return_to: NodeKey::try_from("after").unwrap(),
            traversal_counts: HashMap::new(),
        };
        let mut frames = vec![Frame::Loop(lf.clone())];
        let top = top_loop_mut(&mut frames).expect("loop frame on top");
        assert_eq!(top.loop_node, lf.loop_node);
    }

    #[test]
    fn top_subgraph_returns_none_for_loop_top() {
        let lf = LoopFrame {
            loop_node: NodeKey::try_from("loop_1").unwrap(),
            config: empty_loop_config(),
            items: vec![],
            current_index: 0,
            attempts_remaining: 0,
            return_to: NodeKey::try_from("after").unwrap(),
            traversal_counts: HashMap::new(),
        };
        let frames = vec![Frame::Loop(lf)];
        assert!(top_subgraph(&frames).is_none());
    }
}
