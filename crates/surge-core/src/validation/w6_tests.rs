use super::w5_tests::{agent_node, edge, graph, success_terminal};
use super::{ValidationErrorKind, warning_w6_end_of_pipeline_verification};
use crate::keys::NodeKey;
use crate::node::LedgerEffect;

/// `(gate, work_nodes)` for every W6 finding the graph raises.
fn w6(graph: &crate::graph::Graph) -> Vec<(NodeKey, usize)> {
    let mut out = Vec::new();
    warning_w6_end_of_pipeline_verification(graph, &mut out);
    out.into_iter()
        .filter_map(|f| match f.kind {
            ValidationErrorKind::EndOfPipelineVerification { gate, work_nodes } => {
                Some((gate, work_nodes))
            },
            _ => None,
        })
        .collect()
}

const NONE: &[LedgerEffect] = &[LedgerEffect::None];
const VERIFIES: &[LedgerEffect] = &[LedgerEffect::Verified];

#[test]
fn b1_one_gate_after_two_work_nodes_warns() {
    let g = graph(
        "impl_1",
        vec![
            agent_node("impl_1", "implementer@1.0", NONE),
            agent_node("impl_2", "implementer@1.0", NONE),
            agent_node("verify_1", "verifier@2.0", VERIFIES),
            success_terminal("done"),
        ],
        vec![
            edge("e1", "impl_1", "o0", "impl_2"),
            edge("e2", "impl_2", "o0", "verify_1"),
            edge("e3", "verify_1", "o0", "done"),
        ],
    );
    assert_eq!(w6(&g), vec![(NodeKey::try_from("verify_1").unwrap(), 2)]);
}

#[test]
fn b2_single_work_node_has_only_one_boundary_to_gate() {
    let g = graph(
        "impl_1",
        vec![
            agent_node("impl_1", "implementer@1.0", NONE),
            agent_node("verify_1", "verifier@2.0", VERIFIES),
            success_terminal("done"),
        ],
        vec![
            edge("e1", "impl_1", "o0", "verify_1"),
            edge("e2", "verify_1", "o0", "done"),
        ],
    );
    assert_eq!(w6(&g), vec![]);
}

#[test]
fn b3_no_gate_at_all_is_w4s_finding_not_this_one() {
    let g = graph(
        "impl_1",
        vec![
            agent_node("impl_1", "implementer@1.0", NONE),
            agent_node("impl_2", "implementer@1.0", NONE),
            agent_node("impl_3", "implementer@1.0", NONE),
            success_terminal("done"),
        ],
        vec![
            edge("e1", "impl_1", "o0", "impl_2"),
            edge("e2", "impl_2", "o0", "impl_3"),
            edge("e3", "impl_3", "o0", "done"),
        ],
    );
    assert_eq!(w6(&g), vec![]);
}

/// Pins W6's scope on the shipped set. Multi-milestone now verifies each
/// task and performs a separate final verification, so it no longer has
/// only one verification boundary.
#[test]
fn w6_names_exactly_the_four_bundled_flows_that_verify_only_at_the_end() {
    let mut warned: Vec<String> = Vec::new();
    for flow in crate::BundledFlows::all() {
        let mut out = Vec::new();
        warning_w6_end_of_pipeline_verification(&flow.graph, &mut out);
        if !out.is_empty() {
            warned.push(flow.name.clone());
        }
    }
    warned.sort_unstable();
    assert_eq!(
        warned,
        vec!["bug-fix", "linear-3", "linear-with-review", "refactor",],
        "W6's blast radius on the bundled set changed"
    );
}

#[test]
fn b4_two_gates_is_boundary_verification_and_stays_silent() {
    let g = graph(
        "impl_1",
        vec![
            agent_node("impl_1", "implementer@1.0", NONE),
            agent_node("check_1", "verifier@2.0", VERIFIES),
            agent_node("impl_2", "implementer@1.0", NONE),
            agent_node("check_2", "verifier@2.0", VERIFIES),
            agent_node("impl_3", "implementer@1.0", NONE),
            success_terminal("done"),
        ],
        vec![
            edge("e1", "impl_1", "o0", "check_1"),
            edge("e2", "check_1", "o0", "impl_2"),
            edge("e3", "impl_2", "o0", "check_2"),
            edge("e4", "check_2", "o0", "impl_3"),
            edge("e5", "impl_3", "o0", "done"),
        ],
    );
    assert_eq!(w6(&g), vec![]);
}
