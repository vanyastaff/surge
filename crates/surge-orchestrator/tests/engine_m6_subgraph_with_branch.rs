//! M6 placeholder: Subgraph containing a Branch node that routes based on
//! a run-memory value produced by an Agent stage inside the subgraph.
//! Full e2e requires mock agent emitting a branching outcome (M7).

#[test]
#[ignore = "M6 subgraph+branch: requires mock agent inside subgraph to emit branch outcome (M7)"]
fn subgraph_with_branch_routes_to_correct_outer_outcome() {
    // M7: build a subgraph with an Agent node followed by a Branch node.
    // Mock agent emits outcome "fast_path". Verify subgraph projects the
    // corresponding outer outcome via SubgraphConfig::outputs.
}
