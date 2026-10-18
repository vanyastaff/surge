//! Predicate evaluator for `BranchConfig::predicates`.
//!
//! Pure function (`evaluate`) backed by a small `PredicateContext` trait so
//! the engine can supply runtime data (artifacts, env vars, file existence,
//! prior outcomes) without coupling the evaluator to engine internals.
//!
//! Missing data makes a leaf return `false`; unavailable semantic provenance
//! returns a typed error. Combinators evaluate in order and short-circuit, so
//! unreachable semantic data is neither read nor validated.

use crate::branch_config::{CompareOp, Predicate};
use crate::execution_recovery::semantic_position::PositionError;
use crate::keys::{NodeKey, OutcomeKey};
use std::path::Path;

/// Runtime data source for predicate evaluation.
pub trait PredicateContext {
    /// Most recent outcome reported for `node`, if any.
    ///
    /// # Errors
    /// Returns an error when the selected record has unavailable provenance.
    fn outcome_of(&self, node: &NodeKey) -> Result<Option<&OutcomeKey>, PositionError>;

    /// Size in bytes of the artifact identified by `name`, if it exists.
    ///
    /// Validates selected provenance before metadata access. A missing artifact
    /// or inaccessible metadata is absence.
    ///
    /// # Errors
    /// Returns an error when the selected artifact has invalid provenance.
    fn artifact_size(&self, name: &str) -> Result<Option<u64>, PositionError>;

    /// Value of environment variable `name`, if defined.
    fn env_var(&self, name: &str) -> Option<String>;

    /// Whether `path` (typically relative to the worktree root) exists.
    fn file_exists(&self, path: &Path) -> bool;
}

/// Evaluate `predicate` against `ctx` in order, preserving short-circuiting.
///
/// Missing data returns `false` from the relevant leaf. Unavailable semantic
/// provenance in a reached leaf propagates without choosing a branch.
///
/// # Errors
/// Returns the selected semantic record's position error from the context.
pub fn evaluate(predicate: &Predicate, ctx: &dyn PredicateContext) -> Result<bool, PositionError> {
    Ok(match predicate {
        Predicate::FileExists { path } => ctx.file_exists(Path::new(path)),
        Predicate::ArtifactSize {
            artifact,
            op,
            value,
        } => ctx
            .artifact_size(artifact)?
            .is_some_and(|actual| compare_u64(actual, *op, *value)),
        Predicate::OutcomeMatches { node, outcome } => ctx
            .outcome_of(node)?
            .is_some_and(|actual| actual == outcome),
        Predicate::EnvVar { name, op, value } => ctx
            .env_var(name)
            .is_some_and(|actual| compare_str(&actual, *op, value)),
        Predicate::And { and } => {
            for predicate in and {
                if !evaluate(predicate, ctx)? {
                    return Ok(false);
                }
            }
            true
        },
        Predicate::Or { or } => {
            for predicate in or {
                if evaluate(predicate, ctx)? {
                    return Ok(true);
                }
            }
            false
        },
        Predicate::Not { not } => !evaluate(not, ctx)?,
    })
}

fn compare_u64(a: u64, op: CompareOp, b: u64) -> bool {
    match op {
        CompareOp::Eq => a == b,
        CompareOp::Ne => a != b,
        CompareOp::Lt => a < b,
        CompareOp::Lte => a <= b,
        CompareOp::Gt => a > b,
        CompareOp::Gte => a >= b,
    }
}

fn compare_str(a: &str, op: CompareOp, b: &str) -> bool {
    match op {
        CompareOp::Eq => a == b,
        CompareOp::Ne => a != b,
        CompareOp::Lt => a < b,
        CompareOp::Lte => a <= b,
        CompareOp::Gt => a > b,
        CompareOp::Gte => a >= b,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::{NodeKey, OutcomeKey};
    use std::collections::HashMap;
    use std::path::PathBuf;

    #[derive(Default)]
    struct MockCtx {
        outcomes: HashMap<NodeKey, OutcomeKey>,
        artifacts: HashMap<String, u64>,
        env: HashMap<String, String>,
        files: Vec<PathBuf>,
    }

    impl PredicateContext for MockCtx {
        fn outcome_of(&self, node: &NodeKey) -> Result<Option<&OutcomeKey>, PositionError> {
            Ok(self.outcomes.get(node))
        }
        fn artifact_size(&self, name: &str) -> Result<Option<u64>, PositionError> {
            Ok(self.artifacts.get(name).copied())
        }
        fn env_var(&self, name: &str) -> Option<String> {
            self.env.get(name).cloned()
        }
        fn file_exists(&self, path: &Path) -> bool {
            self.files.iter().any(|p| p == path)
        }
    }

    #[test]
    fn file_exists_true_when_present() {
        let mut ctx = MockCtx::default();
        ctx.files.push(PathBuf::from("Cargo.toml"));
        let p = Predicate::FileExists {
            path: "Cargo.toml".into(),
        };
        assert!(evaluate(&p, &ctx).unwrap());
    }

    #[test]
    fn file_exists_false_when_absent() {
        let ctx = MockCtx::default();
        let p = Predicate::FileExists {
            path: "missing.toml".into(),
        };
        assert!(!evaluate(&p, &ctx).unwrap());
    }

    #[test]
    fn artifact_size_eq() {
        let mut ctx = MockCtx::default();
        ctx.artifacts.insert("spec.md".into(), 1024);
        let p = Predicate::ArtifactSize {
            artifact: "spec.md".into(),
            op: CompareOp::Eq,
            value: 1024,
        };
        assert!(evaluate(&p, &ctx).unwrap());
    }

    #[test]
    fn artifact_size_gt_with_missing_artifact_is_false() {
        let ctx = MockCtx::default();
        let p = Predicate::ArtifactSize {
            artifact: "missing".into(),
            op: CompareOp::Gt,
            value: 0,
        };
        assert!(!evaluate(&p, &ctx).unwrap());
    }

    #[test]
    fn artifact_size_all_compare_ops() {
        let mut ctx = MockCtx::default();
        ctx.artifacts.insert("a".into(), 10);
        for (op, expected) in [
            (CompareOp::Eq, false),
            (CompareOp::Ne, true),
            (CompareOp::Lt, true),
            (CompareOp::Lte, true),
            (CompareOp::Gt, false),
            (CompareOp::Gte, false),
        ] {
            let p = Predicate::ArtifactSize {
                artifact: "a".into(),
                op,
                value: 20,
            };
            assert_eq!(evaluate(&p, &ctx).unwrap(), expected, "op={op:?}");
        }
    }

    #[test]
    fn outcome_matches_positive() {
        let mut ctx = MockCtx::default();
        let n = NodeKey::try_from("plan").unwrap();
        ctx.outcomes
            .insert(n.clone(), OutcomeKey::try_from("done").unwrap());
        let p = Predicate::OutcomeMatches {
            node: n,
            outcome: OutcomeKey::try_from("done").unwrap(),
        };
        assert!(evaluate(&p, &ctx).unwrap());
    }

    #[test]
    fn outcome_matches_missing_node_is_false() {
        let ctx = MockCtx::default();
        let p = Predicate::OutcomeMatches {
            node: NodeKey::try_from("nope").unwrap(),
            outcome: OutcomeKey::try_from("done").unwrap(),
        };
        assert!(!evaluate(&p, &ctx).unwrap());
    }

    #[test]
    fn env_var_eq() {
        let mut ctx = MockCtx::default();
        ctx.env.insert("MODE".into(), "dev".into());
        let p = Predicate::EnvVar {
            name: "MODE".into(),
            op: CompareOp::Eq,
            value: "dev".into(),
        };
        assert!(evaluate(&p, &ctx).unwrap());
    }

    #[test]
    fn env_var_undefined_is_false() {
        let ctx = MockCtx::default();
        let p = Predicate::EnvVar {
            name: "UNDEFINED".into(),
            op: CompareOp::Eq,
            value: "x".into(),
        };
        assert!(!evaluate(&p, &ctx).unwrap());
    }

    #[test]
    fn and_short_circuits_on_first_false() {
        let ctx = MockCtx::default();
        let p = Predicate::And {
            and: vec![
                Predicate::FileExists {
                    path: "missing1".into(),
                },
                Predicate::FileExists {
                    path: "missing2".into(),
                },
            ],
        };
        assert!(!evaluate(&p, &ctx).unwrap());
    }

    #[test]
    fn or_short_circuits_on_first_true() {
        let mut ctx = MockCtx::default();
        ctx.files.push(PathBuf::from("present"));
        let p = Predicate::Or {
            or: vec![
                Predicate::FileExists {
                    path: "present".into(),
                },
                Predicate::FileExists {
                    path: "absent".into(),
                },
            ],
        };
        assert!(evaluate(&p, &ctx).unwrap());
    }

    #[test]
    fn not_inverts_inner() {
        let ctx = MockCtx::default();
        let p = Predicate::Not {
            not: Box::new(Predicate::FileExists {
                path: "missing".into(),
            }),
        };
        assert!(evaluate(&p, &ctx).unwrap());
    }

    #[test]
    fn nested_combinators() {
        let mut ctx = MockCtx::default();
        ctx.files.push(PathBuf::from("a"));
        ctx.artifacts.insert("art".into(), 5);

        // (file_exists("a") AND artifact_size("art") > 0) OR file_exists("z")
        let p = Predicate::Or {
            or: vec![
                Predicate::And {
                    and: vec![
                        Predicate::FileExists { path: "a".into() },
                        Predicate::ArtifactSize {
                            artifact: "art".into(),
                            op: CompareOp::Gt,
                            value: 0,
                        },
                    ],
                },
                Predicate::FileExists { path: "z".into() },
            ],
        };
        assert!(evaluate(&p, &ctx).unwrap());
    }
    struct RefusingCtx {
        calls: std::cell::Cell<usize>,
    }

    impl PredicateContext for RefusingCtx {
        fn outcome_of(&self, _: &NodeKey) -> Result<Option<&OutcomeKey>, PositionError> {
            self.calls.set(self.calls.get() + 1);
            Err(PositionError::ForeignLineage)
        }
        fn artifact_size(&self, _: &str) -> Result<Option<u64>, PositionError> {
            self.calls.set(self.calls.get() + 1);
            Err(PositionError::Unrecorded)
        }
        fn env_var(&self, _: &str) -> Option<String> {
            None
        }
        fn file_exists(&self, path: &Path) -> bool {
            path == Path::new("present")
        }
    }

    #[test]
    fn semantic_errors_propagate_through_reached_combinators() {
        let leaf = Predicate::OutcomeMatches {
            node: NodeKey::try_from("producer").unwrap(),
            outcome: OutcomeKey::try_from("passed").unwrap(),
        };
        for predicate in [
            leaf.clone(),
            Predicate::Not {
                not: Box::new(leaf.clone()),
            },
            Predicate::And {
                and: vec![
                    Predicate::FileExists {
                        path: "present".into(),
                    },
                    leaf.clone(),
                ],
            },
            Predicate::Or {
                or: vec![
                    Predicate::FileExists {
                        path: "absent".into(),
                    },
                    leaf.clone(),
                ],
            },
            Predicate::Not {
                not: Box::new(Predicate::Or { or: vec![leaf] }),
            },
        ] {
            let ctx = RefusingCtx {
                calls: std::cell::Cell::new(0),
            };
            assert_eq!(
                evaluate(&predicate, &ctx),
                Err(PositionError::ForeignLineage)
            );
            assert_eq!(ctx.calls.get(), 1);
        }
        let ctx = RefusingCtx {
            calls: std::cell::Cell::new(0),
        };
        let artifact = Predicate::Not {
            not: Box::new(Predicate::ArtifactSize {
                artifact: "evidence".into(),
                op: CompareOp::Gt,
                value: 0,
            }),
        };
        assert_eq!(evaluate(&artifact, &ctx), Err(PositionError::Unrecorded));
        assert_eq!(ctx.calls.get(), 1);
    }

    #[test]
    fn unreachable_semantic_data_and_empty_combinators_do_not_lookup() {
        let leaf = Predicate::ArtifactSize {
            artifact: "invalid".into(),
            op: CompareOp::Gt,
            value: 0,
        };
        for (predicate, expected) in [
            (
                Predicate::And {
                    and: vec![
                        Predicate::FileExists {
                            path: "absent".into(),
                        },
                        leaf.clone(),
                    ],
                },
                false,
            ),
            (
                Predicate::Or {
                    or: vec![
                        Predicate::FileExists {
                            path: "present".into(),
                        },
                        leaf,
                    ],
                },
                true,
            ),
            (Predicate::And { and: vec![] }, true),
            (Predicate::Or { or: vec![] }, false),
        ] {
            let ctx = RefusingCtx {
                calls: std::cell::Cell::new(0),
            };
            assert_eq!(evaluate(&predicate, &ctx), Ok(expected));
            assert_eq!(ctx.calls.get(), 0);
        }
    }
}
