//! M6 placeholder: iterable loop (IterableSource::Artifact) requires a
//! completed agent stage to produce the artifact before the loop runs.
//! That path needs a mock agent emitting per-iteration outcomes — M7 scope.
//!
//! At unit level the artifact resolution is covered in
//! `engine::stage::loop_stage` (IterableSource::Artifact path) and
//! `engine::stage::bindings` tests.

#[test]
#[ignore = "M6 iterable loop: requires mock_acp_agent emitting per-stage artifact (M7 scope)"]
fn iterable_loop_resolves_artifact_and_iterates() {
    // M7: wire a graph with an Agent node producing a JSON artifact followed
    // by a Loop node with IterableSource::Artifact pointing to that node.
    // Verify the loop iterates once per array element in the artifact.
}
