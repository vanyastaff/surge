//! `NodeKind::Branch` execution. Pure routing logic — no ACP session.

use crate::engine::predicates::EnginePredicateContext;
use crate::engine::stage::{StageError, StageResult};
use std::path::Path;
use surge_core::branch_config::BranchConfig;
use surge_core::keys::NodeKey;
use surge_core::predicate::evaluate;
use surge_core::run_event::{EventPayload, VersionedEventPayload};
use surge_core::run_state::RunMemory;
use surge_persistence::runs::run_writer::RunWriter;

/// Parameters for executing a single `NodeKind::Branch` stage.
pub struct BranchStageParams<'a> {
    /// Key of the branch node being evaluated.
    pub node: &'a NodeKey,
    /// Branch configuration: predicate arms and default outcome.
    pub branch_config: &'a BranchConfig,
    /// Run writer for persisting `OutcomeReported` events.
    pub writer: &'a RunWriter,
    /// Accumulated run memory used to evaluate predicates.
    pub run_memory: &'a RunMemory,
    /// Absolute path to the isolated git worktree (used for `FileExists` predicates).
    pub worktree_root: &'a Path,
}

/// Execute a single `NodeKind::Branch` stage.
///
/// Evaluates each predicate arm in order; the first matching arm's outcome is
/// returned. If no arm matches, the `default_outcome` is returned.
/// A reached semantic provenance error returns before either outcome is recorded.
pub async fn execute_branch_stage(p: BranchStageParams<'_>) -> StageResult {
    let ctx = EnginePredicateContext {
        run_memory: p.run_memory,
        worktree_root: p.worktree_root,
    };

    for arm in &p.branch_config.predicates {
        if evaluate(&arm.condition, &ctx)? {
            p.writer
                .append_event(VersionedEventPayload::new(EventPayload::OutcomeReported {
                    node: p.node.clone(),
                    outcome: arm.outcome.clone(),
                    summary: format!("branch matched arm with outcome={}", arm.outcome),
                }))
                .await
                .map_err(|e| StageError::Storage(e.to_string()))?;
            return Ok(arm.outcome.clone());
        }
    }

    p.writer
        .append_event(VersionedEventPayload::new(EventPayload::OutcomeReported {
            node: p.node.clone(),
            outcome: p.branch_config.default_outcome.clone(),
            summary: "branch fell through to default_outcome".into(),
        }))
        .await
        .map_err(|e| StageError::Storage(e.to_string()))?;

    Ok(p.branch_config.default_outcome.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use surge_core::branch_config::{BranchArm, Predicate};
    use surge_core::keys::OutcomeKey;
    use surge_persistence::runs::Storage;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn matching_arm_wins() {
        let dir = crate::runtime_home_fixture::FixtureHome::new().unwrap();
        let storage = Storage::open(dir.path()).await.unwrap();
        let writer = storage
            .create_run(surge_core::id::RunId::new(), dir.path(), None)
            .await
            .unwrap();

        std::fs::write(dir.path().join("Cargo.toml"), "x").unwrap();

        let cfg = BranchConfig {
            predicates: vec![BranchArm {
                condition: Predicate::FileExists {
                    path: "Cargo.toml".into(),
                },
                outcome: OutcomeKey::try_from("rust").unwrap(),
            }],
            default_outcome: OutcomeKey::try_from("generic").unwrap(),
        };

        let mem = RunMemory::default();
        let node = NodeKey::try_from("decide").unwrap();
        let outcome = execute_branch_stage(BranchStageParams {
            node: &node,
            branch_config: &cfg,
            writer: &writer,
            run_memory: &mem,
            worktree_root: dir.path(),
        })
        .await
        .unwrap();
        assert_eq!(outcome.as_ref(), "rust");
        writer.close().await.unwrap();
        drop(storage);
        dir.close().unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn no_match_falls_back_to_default() {
        let dir = crate::runtime_home_fixture::FixtureHome::new().unwrap();
        let storage = Storage::open(dir.path()).await.unwrap();
        let writer = storage
            .create_run(surge_core::id::RunId::new(), dir.path(), None)
            .await
            .unwrap();

        let cfg = BranchConfig {
            predicates: vec![BranchArm {
                condition: Predicate::FileExists {
                    path: "missing".into(),
                },
                outcome: OutcomeKey::try_from("rust").unwrap(),
            }],
            default_outcome: OutcomeKey::try_from("generic").unwrap(),
        };

        let mem = RunMemory::default();
        let node = NodeKey::try_from("decide").unwrap();
        let outcome = execute_branch_stage(BranchStageParams {
            node: &node,
            branch_config: &cfg,
            writer: &writer,
            run_memory: &mem,
            worktree_root: dir.path(),
        })
        .await
        .unwrap();
        assert_eq!(outcome.as_ref(), "generic");
        writer.close().await.unwrap();
        drop(storage);
        dir.close().unwrap();
    }

    #[derive(Debug, Clone, Copy)]
    enum Probe {
        Outcome,
        Artifact,
    }
    #[derive(Debug, Clone, Copy)]
    enum Form {
        Direct,
        NotNonmatch,
        NotMatch,
        LateArm,
    }

    fn provenance_config(probe: Probe, form: Form) -> BranchConfig {
        use surge_core::branch_config::CompareOp;
        let leaf = |nonmatch| match probe {
            Probe::Outcome => Predicate::OutcomeMatches {
                node: NodeKey::try_from("producer").unwrap(),
                outcome: OutcomeKey::try_from(if nonmatch { "other" } else { "passed" }).unwrap(),
            },
            Probe::Artifact => Predicate::ArtifactSize {
                artifact: "evidence".into(),
                op: if nonmatch {
                    CompareOp::Eq
                } else {
                    CompareOp::Gt
                },
                value: 0,
            },
        };
        let condition = match form {
            Form::Direct | Form::LateArm => leaf(false),
            Form::NotNonmatch => Predicate::Not {
                not: Box::new(leaf(true)),
            },
            Form::NotMatch => Predicate::Not {
                not: Box::new(leaf(false)),
            },
        };
        let mut predicates = Vec::new();
        if matches!(form, Form::LateArm) {
            predicates.push(BranchArm {
                condition: Predicate::FileExists {
                    path: "absent".into(),
                },
                outcome: OutcomeKey::try_from("never").unwrap(),
            });
        }
        predicates.push(BranchArm {
            condition,
            outcome: OutcomeKey::try_from("selected").unwrap(),
        });
        BranchConfig {
            predicates,
            default_outcome: OutcomeKey::try_from("fallback").unwrap(),
        }
    }

    async fn journal_bytes(writer: &RunWriter) -> Vec<u8> {
        let last = writer.current_seq().await.unwrap();
        let events = writer
            .read_events(surge_persistence::runs::EventSeq::ZERO..last.next())
            .await
            .unwrap();
        let rows: Vec<_> = events.iter().map(|event| serde_json::json!({
            "seq": event.seq.as_u64(), "timestamp_ms": event.timestamp_ms, "payload": event.payload,
        })).collect();
        serde_json::to_vec(&rows).unwrap()
    }

    async fn check_branch_provenance(probe: Probe, positives: bool, present: bool) {
        use crate::engine::predicates::provenance_test_support::{cases, memory};
        let home = crate::runtime_home_fixture::FixtureHome::new().unwrap();
        let storage = Storage::open(home.path()).await.unwrap();
        let path = home.path().join("evidence");
        if present {
            std::fs::write(&path, b"evidence").unwrap();
        }
        let node = NodeKey::try_from("decide").unwrap();
        let mut mismatches = Vec::new();
        for case in cases(matches!(probe, Probe::Artifact))
            .into_iter()
            .filter(|case| case.error.is_none() == positives)
        {
            for form in [
                Form::Direct,
                Form::NotNonmatch,
                Form::NotMatch,
                Form::LateArm,
            ] {
                let run = surge_core::RunId::new();
                let writer = storage.create_run(run, home.path(), None).await.unwrap();
                writer
                    .append_event(VersionedEventPayload::new(EventPayload::StageEntered {
                        node: node.clone(),
                        attempt: 1,
                    }))
                    .await
                    .unwrap();
                let before = journal_bytes(&writer).await;
                let memory = memory(&case, &path);
                let cfg = provenance_config(probe, form);
                let result = execute_branch_stage(BranchStageParams {
                    node: &node,
                    branch_config: &cfg,
                    writer: &writer,
                    run_memory: &memory,
                    worktree_root: home.path(),
                })
                .await;
                let after = journal_bytes(&writer).await;
                let correct = if positives {
                    let base = matches!(probe, Probe::Outcome) || present;
                    let selected = match form {
                        Form::Direct | Form::LateArm => base,
                        Form::NotNonmatch => true,
                        Form::NotMatch => !base,
                    };
                    let expected =
                        OutcomeKey::try_from(if selected { "selected" } else { "fallback" })
                            .unwrap();
                    let rows: Vec<serde_json::Value> = serde_json::from_slice(&after).unwrap();
                    let payload = serde_json::to_value(VersionedEventPayload::new(
                        EventPayload::OutcomeReported {
                            node: node.clone(),
                            outcome: expected.clone(),
                            summary: if selected {
                                "branch matched arm with outcome=selected".into()
                            } else {
                                "branch fell through to default_outcome".into()
                            },
                        },
                    ))
                    .unwrap();
                    matches!(&result, Ok(actual) if actual == &expected)
                        && rows.len() == 2
                        && rows[1]["payload"] == payload
                } else {
                    matches!(&result, Err(StageError::SemanticPosition(actual)) if Some(actual) == case.error.as_ref())
                        && before == after
                };
                if !correct {
                    mismatches.push(format!(
                        "{probe:?}/{form:?}/{}: got {result:?}; journal unchanged={}",
                        case.label,
                        before == after
                    ));
                }
                writer.close().await.unwrap();
            }
        }
        drop(storage);
        home.close().unwrap();
        assert!(mismatches.is_empty(), "{mismatches:#?}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn predicate_branch_refuses_outcome_provenance_without_publication() {
        check_branch_provenance(Probe::Outcome, false, true).await;
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn predicate_branch_refuses_artifact_provenance_without_publication() {
        check_branch_provenance(Probe::Artifact, false, true).await;
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn predicate_branch_provenance_precedes_missing_metadata() {
        check_branch_provenance(Probe::Artifact, false, false).await;
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn predicate_branch_local_and_inherited_outcomes_are_exact() {
        check_branch_provenance(Probe::Outcome, true, true).await;
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn predicate_branch_local_and_inherited_artifacts_are_exact() {
        check_branch_provenance(Probe::Artifact, true, true).await;
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn predicate_branch_valid_provenance_missing_bytes_keeps_absence_semantics() {
        check_branch_provenance(Probe::Artifact, true, false).await;
    }
    async fn check_branch_absence_and_earlier_match() {
        use crate::engine::predicates::provenance_test_support::{cases, memory};
        let home = crate::runtime_home_fixture::FixtureHome::new().unwrap();
        let storage = Storage::open(home.path()).await.unwrap();
        std::fs::write(home.path().join("present"), b"x").unwrap();
        let foreign = cases(false)
            .into_iter()
            .find(|case| case.label == "foreign run")
            .unwrap();
        let invalid = memory(&foreign, &home.path().join("evidence"));
        let empty = RunMemory::default();
        let node = NodeKey::try_from("decide").unwrap();
        let mut mismatches = Vec::new();
        for probe in [Probe::Outcome, Probe::Artifact] {
            for mode in 0..3 {
                let mut cfg = provenance_config(probe, Form::Direct);
                let mem = if mode == 0 {
                    cfg.predicates.insert(
                        0,
                        BranchArm {
                            condition: Predicate::FileExists {
                                path: "present".into(),
                            },
                            outcome: OutcomeKey::try_from("early").unwrap(),
                        },
                    );
                    &invalid
                } else {
                    if mode == 2 {
                        cfg.predicates[0].condition = Predicate::Not {
                            not: Box::new(cfg.predicates[0].condition.clone()),
                        };
                    }
                    &empty
                };
                let expected = OutcomeKey::try_from(match mode {
                    0 => "early",
                    1 => "fallback",
                    _ => "selected",
                })
                .unwrap();
                let writer = storage
                    .create_run(surge_core::RunId::new(), home.path(), None)
                    .await
                    .unwrap();
                let result = execute_branch_stage(BranchStageParams {
                    node: &node,
                    branch_config: &cfg,
                    writer: &writer,
                    run_memory: mem,
                    worktree_root: home.path(),
                })
                .await;
                let bytes = journal_bytes(&writer).await;
                let rows: Vec<serde_json::Value> = serde_json::from_slice(&bytes).unwrap();
                let expected_payload = serde_json::to_value(VersionedEventPayload::new(
                    EventPayload::OutcomeReported {
                        node: node.clone(),
                        outcome: expected.clone(),
                        summary: if mode == 1 {
                            "branch fell through to default_outcome".into()
                        } else {
                            format!("branch matched arm with outcome={expected}")
                        },
                    },
                ))
                .unwrap();
                if !matches!(&result, Ok(actual) if actual == &expected)
                    || rows.len() != 1
                    || rows[0]["payload"] != expected_payload
                {
                    mismatches.push(format!("{probe:?}/{mode}: {result:?}, {rows:?}"));
                }
                writer.close().await.unwrap();
            }
        }
        drop(storage);
        home.close().unwrap();
        assert!(mismatches.is_empty(), "{mismatches:#?}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn predicate_branch_absence_not_and_earlier_match_preserve_exact_decisions() {
        check_branch_absence_and_earlier_match().await;
    }
}
