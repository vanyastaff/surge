//! Engine impl of `surge_core::predicate::PredicateContext`.

use std::path::Path;
use surge_core::execution_recovery::semantic_position::PositionError;
use surge_core::keys::{NodeKey, OutcomeKey};
use surge_core::predicate::PredicateContext;
use surge_core::run_state::RunMemory;

/// Engine implementation of [`PredicateContext`] backed by live [`RunMemory`].
pub struct EnginePredicateContext<'a> {
    /// Accumulated run memory: outcomes, artifacts, costs.
    pub run_memory: &'a RunMemory,
    /// Absolute path to the isolated git worktree for this run.
    pub worktree_root: &'a Path,
}

impl PredicateContext for EnginePredicateContext<'_> {
    fn outcome_of(&self, node: &NodeKey) -> Result<Option<&OutcomeKey>, PositionError> {
        let Some(record) = self
            .run_memory
            .outcomes
            .get(node)
            .and_then(|records| records.last())
        else {
            return Ok(None);
        };
        record
            .seq
            .validate(self.run_memory.semantic_lineage.as_ref())?;
        Ok(Some(&record.outcome))
    }

    fn artifact_size(&self, name: &str) -> Result<Option<u64>, PositionError> {
        let Some(artifact) = self.run_memory.artifacts.get(name) else {
            return Ok(None);
        };
        artifact
            .recorded_position()?
            .validate(self.run_memory.semantic_lineage.as_ref())?;
        Ok(std::fs::metadata(&artifact.path)
            .ok()
            .map(|metadata| metadata.len()))
    }

    fn env_var(&self, name: &str) -> Option<String> {
        std::env::var(name).ok()
    }

    fn file_exists(&self, path: &Path) -> bool {
        let abs = self.worktree_root.join(path);
        abs.exists()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use surge_core::branch_config::Predicate;
    use surge_core::keys::NodeKey;
    use surge_core::predicate::evaluate;
    use surge_core::run_state::{OutcomeRecord, RunMemory};

    #[test]
    fn outcome_of_returns_latest() {
        let mut mem = RunMemory::default();
        let node = NodeKey::try_from("plan").unwrap();
        mem.outcomes
            .entry(node.clone())
            .or_default()
            .push(OutcomeRecord {
                outcome: OutcomeKey::try_from("first").unwrap(),
                summary: String::new(),
                seq: surge_core::execution_recovery::semantic_position::SemanticPosition::local(1)
                    .unwrap(),
            });
        mem.outcomes
            .entry(node.clone())
            .or_default()
            .push(OutcomeRecord {
                outcome: OutcomeKey::try_from("second").unwrap(),
                summary: String::new(),
                seq: surge_core::execution_recovery::semantic_position::SemanticPosition::local(2)
                    .unwrap(),
            });
        let ctx = EnginePredicateContext {
            run_memory: &mem,
            worktree_root: Path::new("/tmp"),
        };
        assert_eq!(
            ctx.outcome_of(&node)
                .unwrap()
                .map(std::convert::AsRef::as_ref),
            Some("second")
        );
    }

    #[test]
    fn file_exists_uses_worktree_root() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "x").unwrap();
        let mem = RunMemory::default();
        let ctx = EnginePredicateContext {
            run_memory: &mem,
            worktree_root: dir.path(),
        };
        assert!(ctx.file_exists(Path::new("Cargo.toml")));
        assert!(!ctx.file_exists(Path::new("missing.toml")));
    }

    #[test]
    fn evaluate_outcome_matches_via_engine_ctx() {
        let mut mem = RunMemory::default();
        let node = NodeKey::try_from("plan").unwrap();
        mem.outcomes
            .entry(node.clone())
            .or_default()
            .push(OutcomeRecord {
                outcome: OutcomeKey::try_from("done").unwrap(),
                summary: String::new(),
                seq: surge_core::execution_recovery::semantic_position::SemanticPosition::local(1)
                    .unwrap(),
            });
        let ctx = EnginePredicateContext {
            run_memory: &mem,
            worktree_root: Path::new("/tmp"),
        };
        let p = Predicate::OutcomeMatches {
            node,
            outcome: OutcomeKey::try_from("done").unwrap(),
        };
        assert!(evaluate(&p, &ctx).unwrap());
    }
    #[test]
    fn semantic_lookups_validate_selected_records_before_missing_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("evidence");
        std::fs::write(&path, b"evidence").unwrap();
        let node = NodeKey::try_from("producer").unwrap();
        for case in super::provenance_test_support::cases(true) {
            let memory = super::provenance_test_support::memory(&case, &path);
            let ctx = EnginePredicateContext {
                run_memory: &memory,
                worktree_root: dir.path(),
            };
            assert_eq!(
                ctx.artifact_size("evidence"),
                case.error.clone().map_or(Ok(Some(8)), Err),
                "{}",
                case.label
            );
            if case.position.is_some() {
                assert_eq!(
                    ctx.outcome_of(&node).map(|value| value.map(AsRef::as_ref)),
                    case.error.clone().map_or(Ok(Some("passed")), Err),
                    "{}",
                    case.label
                );
            } else {
                assert_eq!(ctx.outcome_of(&node), Ok(None));
            }
            let missing_memory =
                super::provenance_test_support::memory(&case, &dir.path().join("missing"));
            let missing = EnginePredicateContext {
                run_memory: &missing_memory,
                worktree_root: dir.path(),
            };
            assert_eq!(
                missing.artifact_size("evidence"),
                case.error.clone().map_or(Ok(None), Err),
                "{}",
                case.label
            );
            assert_eq!(ctx.artifact_size("absent"), Ok(None));
            assert_eq!(
                ctx.outcome_of(&NodeKey::try_from("absent").unwrap()),
                Ok(None)
            );
        }
    }

    #[test]
    fn outcome_selection_validates_only_last_record_without_fallback() {
        let cases = super::provenance_test_support::cases(false);
        let local = &cases[0];
        let foreign = cases
            .iter()
            .find(|case| case.label == "foreign run")
            .unwrap();
        let node = NodeKey::try_from("producer").unwrap();
        let mut memory = super::provenance_test_support::memory(local, Path::new("unused"));
        let foreign_memory = super::provenance_test_support::memory(foreign, Path::new("unused"));
        let record = foreign_memory.outcomes[&node][0].clone();
        memory.outcomes.get_mut(&node).unwrap().push(record);
        let ctx = EnginePredicateContext {
            run_memory: &memory,
            worktree_root: Path::new("unused"),
        };
        assert_eq!(ctx.outcome_of(&node), Err(PositionError::ForeignLineage));
        memory.outcomes.get_mut(&node).unwrap().reverse();
        let ctx = EnginePredicateContext {
            run_memory: &memory,
            worktree_root: Path::new("unused"),
        };
        assert_eq!(
            ctx.outcome_of(&node).unwrap().map(AsRef::as_ref),
            Some("passed")
        );
        memory.outcomes.insert(node.clone(), Vec::new());
        let ctx = EnginePredicateContext {
            run_memory: &memory,
            worktree_root: Path::new("unused"),
        };
        assert_eq!(ctx.outcome_of(&node), Ok(None));
    }
}

/// Fixed structural fixtures only; these do not authenticate or hydrate a seed.
#[cfg(test)]
pub(crate) mod provenance_test_support {
    use std::str::FromStr;
    use surge_core::execution_recovery::semantic_position::{
        ContinuationSeedId, PositionError, SeedLineage, SemanticPosition, SourceEventRef,
    };
    pub(crate) struct Case {
        pub label: &'static str,
        pub position: Option<SemanticPosition>,
        pub lineage: Option<SeedLineage>,
        pub error: Option<PositionError>,
    }
    pub(crate) fn cases(unrecorded: bool) -> Vec<Case> {
        let source = surge_core::RunId::from_str("run-00000000000000000000000001").unwrap();
        let other = surge_core::RunId::from_str("run-00000000000000000000000002").unwrap();
        let seed = ContinuationSeedId::new(surge_core::ContentHash::compute(b"predicate seed"));
        let foreign = ContinuationSeedId::new(surge_core::ContentHash::compute(b"foreign seed"));
        let lineage = Some(SeedLineage::new(
            seed,
            SourceEventRef::new(source, 100).unwrap(),
        ));
        let inherited = |seed, source, seq| {
            Some(SemanticPosition::inherited(
                seed,
                SourceEventRef::new(source, seq).unwrap(),
            ))
        };
        let mut result = vec![
            Case {
                label: "local7",
                position: Some(SemanticPosition::local(7).unwrap()),
                lineage: None,
                error: None,
            },
            Case {
                label: "source87",
                position: inherited(seed, source, 87),
                lineage,
                error: None,
            },
            Case {
                label: "missing lineage",
                position: inherited(seed, source, 87),
                lineage: None,
                error: Some(PositionError::ForeignLineage),
            },
            Case {
                label: "foreign seed",
                position: inherited(foreign, source, 87),
                lineage,
                error: Some(PositionError::ForeignLineage),
            },
            Case {
                label: "foreign run",
                position: inherited(seed, other, 87),
                lineage,
                error: Some(PositionError::ForeignLineage),
            },
            Case {
                label: "beyond101",
                position: inherited(seed, source, 101),
                lineage,
                error: Some(PositionError::ForeignLineage),
            },
        ];
        if unrecorded {
            result.push(Case {
                label: "unrecorded",
                position: None,
                lineage,
                error: Some(PositionError::Unrecorded),
            });
        }
        result
    }
    pub(crate) fn memory(case: &Case, path: &std::path::Path) -> surge_core::run_state::RunMemory {
        use surge_core::run_state::{ArtifactRef, OutcomeRecord, RunMemory};
        let producer = surge_core::NodeKey::try_from("producer").unwrap();
        let mut memory = RunMemory {
            semantic_lineage: case.lineage,
            ..RunMemory::default()
        };
        if let Some(seq) = case.position {
            memory.outcomes.insert(
                producer.clone(),
                vec![OutcomeRecord {
                    outcome: surge_core::OutcomeKey::try_from("passed").unwrap(),
                    summary: "fixed source evidence".into(),
                    seq,
                }],
            );
        }
        memory.artifacts.insert(
            "evidence".into(),
            ArtifactRef {
                hash: surge_core::ContentHash::compute(b"evidence"),
                path: path.to_path_buf(),
                name: "evidence".into(),
                produced_by: producer,
                produced_at_seq: case.position,
            },
        );
        memory
    }
}
