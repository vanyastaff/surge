//! M6 placeholder: crash-resume e2e with an active SubgraphFrame in the snapshot.
//! Full crash-resume requires daemon kill semantics (kill process inside an
//! inner subgraph, restart, verify SubgraphFrame + inner cursor restored).
//! Frame serialization is covered at unit level in engine::snapshot tests.

#[test]
#[ignore = "M6 crash-resume inside subgraph: requires daemon-mode kill semantics (M7)"]
fn resume_after_crash_inside_subgraph_restores_subgraph_frame() {
    // M7: start a run that enters a Subgraph, kill the engine process, restart,
    // call engine.resume_run(), verify:
    // (a) SubgraphFrame is on the frame stack with correct outer_node and return_to.
    // (b) The cursor is inside the inner subgraph, not at the outer graph start.
}
