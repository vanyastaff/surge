//! Human-readable review of a generated `flow.toml`.
//!
//! The flow gate used to show the raw TOML through the Markdown renderer:
//! comment banners became giant headings and every `key = value` line became
//! prose. An operator approving the plan needs the steps, who does each one
//! and where their approvals are; the raw file stays available underneath.

use std::collections::{BTreeMap, HashSet, VecDeque};
use std::fmt::Write as _;

use surge_core::edge::Edge;
use surge_core::graph::{Graph, Subgraph};
use surge_core::keys::{NodeKey, SubgraphKey};
use surge_core::node::{Node, NodeConfig};
use surge_core::terminal_config::TerminalKind;

/// Markdown for the flow review pane: a step summary, then the source.
pub fn flow_review_markdown(source: &str) -> String {
    let mut out = String::new();
    match toml::from_str::<Graph>(source) {
        Ok(graph) => {
            let steps = count_steps(&graph);
            let _ = writeln!(
                out,
                "**{} steps** · {} by an agent · {} need your approval\n",
                steps.total, steps.agent, steps.approvals
            );
            write_steps(
                &mut out,
                &graph.start,
                &graph.nodes,
                &graph.edges,
                &graph.subgraphs,
                0,
            );
        },
        Err(error) => {
            let _ = writeln!(out, "This workflow could not be parsed: {error}\n");
        },
    }
    let _ = write!(
        out,
        "\n#### flow.toml\n\n```toml\n{}\n```\n",
        source.trim_end()
    );
    out
}

#[derive(Default)]
struct StepCount {
    total: usize,
    agent: usize,
    approvals: usize,
}

fn count_steps(graph: &Graph) -> StepCount {
    let mut count = StepCount::default();
    let nodes = graph
        .nodes
        .values()
        .chain(graph.subgraphs.values().flat_map(|s| s.nodes.values()));
    for node in nodes {
        match &node.config {
            NodeConfig::Terminal(_) => continue,
            NodeConfig::Agent(_) => count.agent += 1,
            NodeConfig::HumanGate(_) => count.approvals += 1,
            _ => {},
        }
        count.total += 1;
    }
    count
}

/// Walk from `start` in edge order (breadth-first) so the list reads in
/// execution order; unreachable nodes are appended so nothing is hidden.
fn write_steps(
    out: &mut String,
    start: &NodeKey,
    nodes: &BTreeMap<NodeKey, Node>,
    edges: &[Edge],
    subgraphs: &BTreeMap<SubgraphKey, Subgraph>,
    depth: usize,
) {
    let mut order = Vec::new();
    let mut seen = HashSet::new();
    let mut queue = VecDeque::from([start.clone()]);
    while let Some(key) = queue.pop_front() {
        if !nodes.contains_key(&key) || !seen.insert(key.clone()) {
            continue;
        }
        order.push(key.clone());
        for edge in edges.iter().filter(|e| e.from.node == key) {
            queue.push_back(edge.to.clone());
        }
    }
    order.extend(nodes.keys().filter(|k| !seen.contains(*k)).cloned());

    let indent = "   ".repeat(depth);
    let mut number = 0;
    for key in order {
        let Some(node) = nodes.get(&key) else {
            continue;
        };
        if let NodeConfig::Terminal(t) = &node.config {
            let end = match t.kind {
                TerminalKind::Success => "finishes successfully",
                TerminalKind::Failure { .. } => "stops as failed",
                TerminalKind::Aborted => "stops as aborted",
            };
            let _ = writeln!(out, "{indent}- `{key}` — {end}");
            continue;
        }
        number += 1;
        let _ = writeln!(out, "{indent}{number}. {}", describe(&key, node));
        if let NodeConfig::Loop(l) = &node.config
            && let Some(body) = subgraphs.get(&l.body)
        {
            write_steps(
                out,
                &body.start,
                &body.nodes,
                &body.edges,
                subgraphs,
                depth + 1,
            );
        }
        if let NodeConfig::Subgraph(s) = &node.config
            && let Some(inner) = subgraphs.get(&s.inner)
        {
            write_steps(
                out,
                &inner.start,
                &inner.nodes,
                &inner.edges,
                subgraphs,
                depth + 1,
            );
        }
    }
}

fn describe(key: &NodeKey, node: &Node) -> String {
    let what = match &node.config {
        NodeConfig::Agent(agent) => format!("agent `{}`", agent.profile),
        NodeConfig::HumanGate(_) => "**your approval**".to_string(),
        NodeConfig::Branch(_) => "automatic decision".to_string(),
        NodeConfig::Notify(_) => "notification".to_string(),
        NodeConfig::Loop(l) => format!("repeat `{}` for each item", l.body),
        NodeConfig::Subgraph(s) => format!("runs sub-flow `{}`", s.inner),
        NodeConfig::Terminal(_) => "end".to_string(),
    };
    let outcomes: Vec<&str> = node
        .declared_outcomes
        .iter()
        .map(|o| o.id.as_str())
        .collect();
    if outcomes.is_empty() {
        format!("**{key}** — {what}")
    } else {
        format!("**{key}** — {what} → {}", outcomes.join(" / "))
    }
}

#[cfg(test)]
mod tests {
    use super::flow_review_markdown;

    #[test]
    fn bundled_flow_reads_as_ordered_steps_with_the_source_below() {
        let source = include_str!("../../../examples/flow_bug_fix.toml");
        let review = flow_review_markdown(source);
        assert!(review.starts_with("**"), "summary first: {review}");
        assert!(review.contains("1. **"), "numbered steps: {review}");
        assert!(
            review.contains("agent `"),
            "names the agent profile: {review}"
        );
        assert!(review.contains("```toml"), "raw source kept: {review}");
        // The comment banners of the raw file must not become headings.
        let summary = review.split("#### flow.toml").next().unwrap_or_default();
        assert!(!summary.contains("# "), "{summary}");
    }

    #[test]
    fn unparsable_flow_still_shows_its_source() {
        let review = flow_review_markdown("not = [valid");
        assert!(review.contains("could not be parsed"));
        assert!(review.contains("not = [valid"));
    }
}
