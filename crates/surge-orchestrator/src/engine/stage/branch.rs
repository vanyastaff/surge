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
pub async fn execute_branch_stage(p: BranchStageParams<'_>) -> StageResult {
    let ctx = EnginePredicateContext {
        run_memory: p.run_memory,
        worktree_root: p.worktree_root,
    };

    for arm in &p.branch_config.predicates {
        if evaluate(&arm.condition, &ctx) {
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
        let dir = tempfile::tempdir().unwrap();
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
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn no_match_falls_back_to_default() {
        let dir = tempfile::tempdir().unwrap();
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
    }
}
