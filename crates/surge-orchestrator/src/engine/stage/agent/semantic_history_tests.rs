//! Post-edit actual-caller coverage; these are not pre-migration RED claims.
use super::*;
use surge_core::execution_recovery::semantic_position::{
    ContinuationSeedId, SeedLineage, SemanticPosition, SourceEventRef,
};
use surge_core::run_state::{
    ArtifactRef, BacktrackFeedback, OutcomeRecord, RequirementRevision, RunMemory,
};
mod runtime_home {
    #[cfg(windows)]
    use surge_persistence::RuntimeHomeOwner;
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../scripts/test-support/runtime_home.rs"
    ));
}

#[tokio::test]
async fn actual_feedback_uses_source_10_report_20_before_edge_25() {
    let home = runtime_home::FixtureHome::new().unwrap();
    let store = ArtifactStore::new(home.path().join("runs"));
    let run = surge_core::RunId::new();
    let source = surge_core::RunId::new();
    let seed = ContinuationSeedId::new(ContentHash::compute(b"fixed-selected-seed"));
    let position =
        |seq| SemanticPosition::inherited(seed, SourceEventRef::new(source, seq).unwrap());
    let verify = NodeKey::try_from("verify").unwrap();
    let build = NodeKey::try_from("build").unwrap();
    let bytes = b"schema_version = 1\ntask_id = 'login'\noutcome = 'failed'\nsummary = 'One required behavior failed.'\n[[checks]]\ncommand = 'node --test login'\nresult = 'failed'\ncovers = ['criterion:1']\nnote = 'Empty name was accepted.'\n";
    let handle = store.put(run, "verification-report", bytes).await.unwrap();
    assert!(
        handle.produced_at_seq.is_none(),
        "put creates a byte handle, not history"
    );
    let mut memory = RunMemory {
        semantic_lineage: Some(SeedLineage::new(
            seed,
            SourceEventRef::new(source, 87).unwrap(),
        )),
        ..RunMemory::default()
    };
    memory.outcomes.insert(
        verify.clone(),
        vec![
            OutcomeRecord {
                outcome: OutcomeKey::try_from("failed").unwrap(),
                summary: "Older unrelated failure.".into(),
                seq: position(5),
            },
            OutcomeRecord {
                outcome: OutcomeKey::try_from("failed").unwrap(),
                summary: "Login validation failed.".into(),
                seq: position(10),
            },
            OutcomeRecord {
                outcome: OutcomeKey::try_from("passed").unwrap(),
                summary: "Future local outcome must not hide source feedback.".into(),
                seq: SemanticPosition::local(5).unwrap(),
            },
        ],
    );
    memory.artifacts_by_node.insert(
        verify.clone(),
        vec![ArtifactRef {
            produced_by: verify.clone(),
            produced_at_seq: Some(position(20)),
            ..handle
        }],
    );
    memory.backtrack_feedback.insert(
        build.clone(),
        BacktrackFeedback {
            from: verify,
            edge_seq: position(25),
        },
    );
    let feedback = previous_attempt_feedback(&build, &memory, &store, run)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        feedback,
        "## Feedback from the previous attempt\nStage `verify` sent this work back with outcome `failed`. Address it before reporting again.\n\nSummary: Login validation failed.\n\nVerifier summary: One required behavior failed.\n\nChecks that did not pass:\n- `node --test login` — failed: Empty name was accepted.\n\n"
    );
    memory.semantic_lineage = Some(SeedLineage::new(
        seed,
        SourceEventRef::new(source, 19).unwrap(),
    ));
    assert!(
        previous_attempt_feedback(&build, &memory, &store, run)
            .await
            .is_err(),
        "beyond-boundary report/edge cannot become feedback"
    );
    memory.semantic_lineage = Some(SeedLineage::new(
        ContinuationSeedId::new(ContentHash::compute(b"foreign-seed")),
        SourceEventRef::new(source, 87).unwrap(),
    ));
    assert!(
        previous_attempt_feedback(&build, &memory, &store, run)
            .await
            .is_err(),
        "foreign seed cannot become feedback"
    );
    memory.semantic_lineage = Some(SeedLineage::new(
        seed,
        SourceEventRef::new(surge_core::RunId::new(), 87).unwrap(),
    ));
    assert!(
        previous_attempt_feedback(&build, &memory, &store, run)
            .await
            .is_err(),
        "foreign source must not become selected feedback"
    );
    drop(store);
    home.close().unwrap();
}

#[test]
fn actual_requirement_prompt_preserves_source_then_local_and_task_scope() {
    let source = surge_core::RunId::new();
    let seed = ContinuationSeedId::new(ContentHash::compute(b"requirements"));
    let lineage = SeedLineage::new(seed, SourceEventRef::new(source, 87).unwrap());
    let task = RoadmapTaskId::from("login");
    let node = NodeKey::try_from("build").unwrap();
    let revisions = [
        RequirementRevision {
            node: node.clone(),
            task: Some(task.clone()),
            text: "Original source requirement.".into(),
            seq: SemanticPosition::inherited(seed, SourceEventRef::new(source, 20).unwrap()),
        },
        RequirementRevision {
            node: node.clone(),
            task: Some(RoadmapTaskId::from("logout")),
            text: "Unrelated task must stay excluded.".into(),
            seq: SemanticPosition::local(2).unwrap(),
        },
        RequirementRevision {
            node,
            task: Some(task.clone()),
            text: "New local requirement wins.".into(),
            seq: SemanticPosition::local(5).unwrap(),
        },
    ];
    let prompt = prepend_requirement_revisions(
        "Implement login.".into(),
        &revisions,
        Some(&task),
        Some(&lineage),
    )
    .unwrap();
    assert_eq!(
        prompt,
        "## Requirement revised by the operator\nThe operator changed the requirement for this work. Where it conflicts with the task or earlier criteria, the latest revision wins:\n- Original source requirement.\n- New local requirement wins.\n\nImplement login."
    );
    assert!(prepend_requirement_revisions("Do it.".into(), &revisions, Some(&task), None).is_err());
}
