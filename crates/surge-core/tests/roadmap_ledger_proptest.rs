//! Property-based tests for the roadmap task-ledger invariants.

use proptest::prelude::*;
use surge_core::{
    RoadmapArtifact, RoadmapLedgerIssue, RoadmapMilestone, RoadmapTask, TaskSize,
};

/// Build a sized task with the given id and dependency indexes.
fn task(id: usize, depends_on: Vec<usize>) -> RoadmapTask {
    let mut task = RoadmapTask::new(format!("t{id}"), format!("Task {id}"));
    task.size = Some(TaskSize::M);
    task.depends_on = depends_on
        .into_iter()
        .map(|dependency| format!("t{dependency}"))
        .collect();
    task
}

fn roadmap_with_tasks(tasks: Vec<RoadmapTask>) -> RoadmapArtifact {
    let mut milestone = RoadmapMilestone::new("m1", "Generated");
    milestone.tasks = tasks;
    RoadmapArtifact::new(vec![milestone])
}

/// Tasks whose dependencies only point at strictly-smaller indexes — a DAG
/// by construction, with every reference valid.
fn arb_dag_tasks() -> impl Strategy<Value = Vec<RoadmapTask>> {
    (1usize..12).prop_flat_map(|count| {
        let deps: Vec<_> = (0..count)
            .map(|index| {
                if index == 0 {
                    proptest::collection::vec(0..1usize, 0..1).boxed()
                } else {
                    proptest::collection::vec(0..index, 0..index.min(4)).boxed()
                }
            })
            .collect();
        deps.prop_map(|per_task| {
            per_task
                .into_iter()
                .enumerate()
                .map(|(index, mut depends_on)| {
                    depends_on.retain(|dependency| *dependency != index);
                    depends_on.sort_unstable();
                    depends_on.dedup();
                    task(index, depends_on)
                })
                .collect()
        })
    })
}

proptest! {
    /// A dependency graph that is a DAG by construction with valid
    /// references and sizes never reports ledger issues.
    #[test]
    fn dag_roadmaps_validate_clean(tasks in arb_dag_tasks()) {
        let roadmap = roadmap_with_tasks(tasks);
        prop_assert_eq!(roadmap.validate_ledger(), Vec::new());
    }

    /// Closing any dependency chain into a ring is always reported as a
    /// cycle, regardless of chain length.
    #[test]
    fn rings_always_report_a_cycle(length in 2usize..10) {
        let tasks: Vec<RoadmapTask> = (0..length)
            .map(|index| task(index, vec![(index + 1) % length]))
            .collect();
        let roadmap = roadmap_with_tasks(tasks);

        let issues = roadmap.validate_ledger();
        prop_assert!(
            issues
                .iter()
                .any(|issue| matches!(issue, RoadmapLedgerIssue::DependencyCycle { .. })),
            "expected a cycle issue, got {issues:?}"
        );
    }

    /// Any dependency on an id outside the task set is reported, and naming
    /// it never panics the validator.
    #[test]
    fn unknown_references_are_always_reported(
        tasks in arb_dag_tasks(),
        victim in any::<prop::sample::Index>(),
    ) {
        let mut tasks = tasks;
        let index = victim.index(tasks.len());
        tasks[index].depends_on.push("missing-task".to_string());
        let victim_id = tasks[index].id.clone();
        let roadmap = roadmap_with_tasks(tasks);

        let issues = roadmap.validate_ledger();
        let expected = RoadmapLedgerIssue::UnknownDependsOn {
            task: victim_id,
            missing: "missing-task".to_string(),
        };
        prop_assert!(issues.contains(&expected), "missing {expected:?} in {issues:?}");
    }

    /// validate_ledger is deterministic: the same roadmap always yields the
    /// same issues in the same order (fold/replay requirement).
    #[test]
    fn validation_is_deterministic(tasks in arb_dag_tasks(), ring in 0usize..4) {
        let mut tasks = tasks;
        // Optionally poison the DAG with a small ring appended at the end.
        if ring >= 2 {
            let base = tasks.len();
            for index in 0..ring {
                tasks.push(task(base + index, vec![base + ((index + 1) % ring)]));
            }
        }
        let roadmap = roadmap_with_tasks(tasks);
        prop_assert_eq!(roadmap.validate_ledger(), roadmap.validate_ledger());
    }
}
