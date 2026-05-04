//! Predicate evaluator for `BranchConfig::predicates`.
//!
//! Pure function (`evaluate`) backed by a small `PredicateContext` trait so
//! the engine can supply runtime data (artifacts, env vars, file existence,
//! prior outcomes) without coupling the evaluator to engine internals.
//!
//! Fail-closed semantics: missing data (unknown artifact name, undefined env
//! var, broken symlink) makes the leaf predicate return `false`. Combinators
//! short-circuit normally. Documented choice — in an autonomous setting,
//! panicking on missing data would turn a small data error into a run-killing
//! crash; falling back keeps the run going and surfaces the divergence via
//! `OutcomeReported.summary`.

use crate::branch_config::{CompareOp, Predicate};
use crate::keys::{NodeKey, OutcomeKey};
use std::path::Path;

/// Runtime data source for predicate evaluation.
pub trait PredicateContext {
    /// Most recent outcome reported for `node`, if any.
    fn outcome_of(&self, node: &NodeKey) -> Option<&OutcomeKey>;

    /// Size in bytes of the artifact identified by `name`, if it exists.
    fn artifact_size(&self, name: &str) -> Option<u64>;

    /// Value of environment variable `name`, if defined.
    fn env_var(&self, name: &str) -> Option<String>;

    /// Whether `path` (typically relative to the worktree root) exists.
    fn file_exists(&self, path: &Path) -> bool;
}

/// Evaluate `predicate` against `ctx`. Never panics; missing data returns
/// `false` from the relevant leaf and short-circuits combinators normally.
#[must_use]
pub fn evaluate(predicate: &Predicate, ctx: &dyn PredicateContext) -> bool {
    match predicate {
        Predicate::FileExists { path } => ctx.file_exists(Path::new(path)),
        Predicate::ArtifactSize { artifact, op, value } => ctx
            .artifact_size(artifact)
            .map(|actual| compare_u64(actual, *op, *value))
            .unwrap_or(false),
        Predicate::OutcomeMatches { node, outcome } => {
            ctx.outcome_of(node).is_some_and(|o| o == outcome)
        }
        Predicate::EnvVar { name, op, value } => ctx
            .env_var(name)
            .map(|actual| compare_str(&actual, *op, value))
            .unwrap_or(false),
        Predicate::And { and } => and.iter().all(|p| evaluate(p, ctx)),
        Predicate::Or { or } => or.iter().any(|p| evaluate(p, ctx)),
        Predicate::Not { not } => !evaluate(not, ctx),
    }
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
        fn outcome_of(&self, node: &NodeKey) -> Option<&OutcomeKey> {
            self.outcomes.get(node)
        }
        fn artifact_size(&self, name: &str) -> Option<u64> {
            self.artifacts.get(name).copied()
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
        let p = Predicate::FileExists { path: "Cargo.toml".into() };
        assert!(evaluate(&p, &ctx));
    }

    #[test]
    fn file_exists_false_when_absent() {
        let ctx = MockCtx::default();
        let p = Predicate::FileExists { path: "missing.toml".into() };
        assert!(!evaluate(&p, &ctx));
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
        assert!(evaluate(&p, &ctx));
    }

    #[test]
    fn artifact_size_gt_with_missing_artifact_is_false() {
        let ctx = MockCtx::default();
        let p = Predicate::ArtifactSize {
            artifact: "missing".into(),
            op: CompareOp::Gt,
            value: 0,
        };
        assert!(!evaluate(&p, &ctx));
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
            assert_eq!(evaluate(&p, &ctx), expected, "op={op:?}");
        }
    }

    #[test]
    fn outcome_matches_positive() {
        let mut ctx = MockCtx::default();
        let n = NodeKey::try_from("plan").unwrap();
        ctx.outcomes.insert(n.clone(), OutcomeKey::try_from("done").unwrap());
        let p = Predicate::OutcomeMatches {
            node: n,
            outcome: OutcomeKey::try_from("done").unwrap(),
        };
        assert!(evaluate(&p, &ctx));
    }

    #[test]
    fn outcome_matches_missing_node_is_false() {
        let ctx = MockCtx::default();
        let p = Predicate::OutcomeMatches {
            node: NodeKey::try_from("nope").unwrap(),
            outcome: OutcomeKey::try_from("done").unwrap(),
        };
        assert!(!evaluate(&p, &ctx));
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
        assert!(evaluate(&p, &ctx));
    }

    #[test]
    fn env_var_undefined_is_false() {
        let ctx = MockCtx::default();
        let p = Predicate::EnvVar {
            name: "UNDEFINED".into(),
            op: CompareOp::Eq,
            value: "x".into(),
        };
        assert!(!evaluate(&p, &ctx));
    }

    #[test]
    fn and_short_circuits_on_first_false() {
        let ctx = MockCtx::default();
        let p = Predicate::And {
            and: vec![
                Predicate::FileExists { path: "missing1".into() },
                Predicate::FileExists { path: "missing2".into() },
            ],
        };
        assert!(!evaluate(&p, &ctx));
    }

    #[test]
    fn or_short_circuits_on_first_true() {
        let mut ctx = MockCtx::default();
        ctx.files.push(PathBuf::from("present"));
        let p = Predicate::Or {
            or: vec![
                Predicate::FileExists { path: "present".into() },
                Predicate::FileExists { path: "absent".into() },
            ],
        };
        assert!(evaluate(&p, &ctx));
    }

    #[test]
    fn not_inverts_inner() {
        let ctx = MockCtx::default();
        let p = Predicate::Not {
            not: Box::new(Predicate::FileExists { path: "missing".into() }),
        };
        assert!(evaluate(&p, &ctx));
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
        assert!(evaluate(&p, &ctx));
    }
}
