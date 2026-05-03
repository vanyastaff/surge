//! Engine impl of `surge_core::predicate::PredicateContext`.

use std::path::Path;
use surge_core::keys::{NodeKey, OutcomeKey};
use surge_core::predicate::PredicateContext;
use surge_core::run_state::RunMemory;

pub struct EnginePredicateContext<'a> {
    pub run_memory: &'a RunMemory,
    pub worktree_root: &'a Path,
}

impl<'a> PredicateContext for EnginePredicateContext<'a> {
    fn outcome_of(&self, node: &NodeKey) -> Option<&OutcomeKey> {
        self.run_memory
            .outcomes
            .get(node)
            .and_then(|recs| recs.last())
            .map(|r| &r.outcome)
    }

    fn artifact_size(&self, name: &str) -> Option<u64> {
        self.run_memory
            .artifacts
            .get(name)
            .and_then(|a| std::fs::metadata(&a.path).ok())
            .map(|m| m.len())
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
        mem.outcomes.entry(node.clone()).or_default().push(OutcomeRecord {
            outcome: OutcomeKey::try_from("first").unwrap(),
            summary: "".into(),
            seq: 1,
        });
        mem.outcomes.entry(node.clone()).or_default().push(OutcomeRecord {
            outcome: OutcomeKey::try_from("second").unwrap(),
            summary: "".into(),
            seq: 2,
        });
        let ctx = EnginePredicateContext {
            run_memory: &mem,
            worktree_root: Path::new("/tmp"),
        };
        assert_eq!(
            ctx.outcome_of(&node).map(|o| o.as_ref()),
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
        mem.outcomes.entry(node.clone()).or_default().push(OutcomeRecord {
            outcome: OutcomeKey::try_from("done").unwrap(),
            summary: "".into(),
            seq: 1,
        });
        let ctx = EnginePredicateContext {
            run_memory: &mem,
            worktree_root: Path::new("/tmp"),
        };
        let p = Predicate::OutcomeMatches {
            node,
            outcome: OutcomeKey::try_from("done").unwrap(),
        };
        assert!(evaluate(&p, &ctx));
    }
}
