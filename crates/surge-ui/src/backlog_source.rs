//! The project's backlog: every task of every mission, where it stands.
//!
//! Tasks come from each mission's approved roadmap (title, milestone,
//! description, acceptance criteria, dependencies); their status comes from
//! the implementation run's task ledger, the same index `surge ready` /
//! `surge ledger` read. Tasks discovered mid-run appear with their origin.
//!
//! Scoping is by the project's bootstrap operations (which record the
//! origin repository), so another project's tasks never show up here — the
//! ledger alone cannot tell projects apart (its rows record worktrees).

use std::collections::HashMap;
use std::path::Path;

use surge_core::roadmap::{RoadmapArtifact, RoadmapStatus};
use surge_core::{EventPayload, RunId};

/// One task on the board.
#[derive(Clone, Debug, PartialEq)]
pub struct BacklogTask {
    pub id: String,
    pub title: String,
    pub description: Option<String>,
    pub acceptance: Vec<String>,
    pub milestone: Option<String>,
    /// The request the mission was started with.
    pub mission: String,
    pub implementation_run: RunId,
    pub status: RoadmapStatus,
    pub verified: bool,
    pub discovered_from: Option<String>,
    /// Unfinished tasks of the same mission this one waits for.
    pub blocked_by: Vec<String>,
    /// The mission's build has ended — nothing will pick this task up
    /// unless you start it again.
    pub mission_ended: bool,
}

/// Ledger view of one task: status + verified.
pub type Ledger = HashMap<String, (RoadmapStatus, bool)>;

fn done(status: RoadmapStatus) -> bool {
    matches!(status, RoadmapStatus::Completed | RoadmapStatus::Skipped)
}

/// Assemble one mission's tasks from its plan, its ledger and the tasks
/// discovered while it ran (`(id, title, discovered_from)`).
pub fn assemble(
    mission: &str,
    implementation_run: RunId,
    roadmap: Option<&RoadmapArtifact>,
    ledger: &Ledger,
    discovered: &[(String, String, String)],
    mission_ended: bool,
) -> Vec<BacklogTask> {
    let mut tasks: Vec<BacklogTask> = Vec::new();
    let mut depends: HashMap<String, Vec<String>> = HashMap::new();
    if let Some(roadmap) = roadmap {
        for m in &roadmap.milestones {
            for t in &m.tasks {
                let (status, verified) = ledger.get(&t.id).copied().unwrap_or((t.status, t.verified));
                depends.insert(t.id.clone(), t.depends_on.clone());
                tasks.push(BacklogTask {
                    id: t.id.clone(),
                    title: t.title.clone(),
                    description: t.description.clone().filter(|d| !d.trim().is_empty()),
                    acceptance: t.acceptance_criteria.clone(),
                    milestone: Some(m.title.clone()),
                    mission: mission.to_string(),
                    implementation_run,
                    status,
                    verified,
                    discovered_from: t.discovered_from.clone(),
                    blocked_by: Vec::new(),
                    mission_ended,
                });
            }
        }
    }
    for (id, title, from) in discovered {
        if tasks.iter().any(|t| &t.id == id) {
            continue;
        }
        let (status, verified) = ledger.get(id).copied().unwrap_or((RoadmapStatus::Pending, false));
        let milestone = tasks.iter().find(|t| &t.id == from).and_then(|t| t.milestone.clone());
        tasks.push(BacklogTask {
            id: id.clone(),
            title: title.clone(),
            description: None,
            acceptance: Vec::new(),
            milestone,
            mission: mission.to_string(),
            implementation_run,
            status,
            verified,
            discovered_from: Some(from.clone()),
            blocked_by: Vec::new(),
            mission_ended,
        });
    }
    let finished: HashMap<String, bool> = tasks.iter().map(|t| (t.id.clone(), done(t.status))).collect();
    for task in &mut tasks {
        if done(task.status) {
            continue;
        }
        task.blocked_by = depends
            .get(&task.id)
            .into_iter()
            .flatten()
            .filter(|dep| !finished.get(*dep).copied().unwrap_or(false))
            .cloned()
            .collect();
    }
    tasks
}

/// Every mission's tasks for `project_root`, newest mission first.
pub async fn load(project_root: &Path, surge_home: &Path) -> Vec<BacklogTask> {
    use surge_persistence::task_ledger::TaskLedgerIndexFilter;

    if tokio::runtime::Handle::try_current().is_err() {
        return Vec::new();
    }
    let operations = crate::roadmap_source::operations(project_root, surge_home).await;
    let storage = surge_persistence::runs::Storage::open(surge_home).await.ok();
    let root = surge_home.join("runs");
    let mut all = Vec::new();
    for op in operations {
        let roadmap: Option<RoadmapArtifact> =
            crate::roadmap_source::read_run_artifact(surge_home, op.planning_run, "roadmap_toml")
                .and_then(|b| String::from_utf8(b).ok())
                .and_then(|t| toml::from_str(&t).ok());
        let ledger: Ledger = storage
            .as_ref()
            .and_then(|s| {
                s.task_ledger_store()
                    .list(&TaskLedgerIndexFilter {
                        status: None,
                        project_path: None,
                        run_id: Some(op.implementation_run),
                        discovered_only: false,
                        limit: None,
                    })
                    .ok()
            })
            .unwrap_or_default()
            .into_iter()
            .map(|row| (row.task_id, (row.status, row.verified)))
            .collect();
        let events: Vec<EventPayload> =
            surge_persistence::runs::Storage::inspect_existing_run_events(root.clone(), op.implementation_run)
                .await
                .map(|e| e.into_iter().map(|e| e.payload.payload).collect())
                .unwrap_or_default();
        let discovered: Vec<(String, String, String)> = events
            .iter()
            .filter_map(|e| match e {
                EventPayload::TaskDiscovered { task_id, discovered_from, title } => {
                    Some((task_id.clone(), title.clone(), discovered_from.clone()))
                },
                _ => None,
            })
            .collect();
        let ended = events.iter().any(|e| {
            matches!(
                e,
                EventPayload::RunCompleted { .. } | EventPayload::RunFailed { .. } | EventPayload::RunAborted { .. }
            )
        });
        // A mission still planning has no build yet; its plan is not work
        // anyone has picked up.
        if roadmap.is_none() && ledger.is_empty() && discovered.is_empty() {
            continue;
        }
        all.extend(assemble(
            &crate::ui::headline(&op.prompt, 90),
            op.implementation_run,
            roadmap.as_ref(),
            &ledger,
            &discovered,
            ended,
        ));
    }
    all
}

#[cfg(test)]
mod tests {
    use super::{Ledger, assemble};
    use surge_core::RunId;
    use surge_core::roadmap::{RoadmapArtifact, RoadmapStatus};

    fn roadmap() -> RoadmapArtifact {
        toml::from_str(
            r#"
schema_version = 2
[[milestones]]
id = "m1"
title = "Core"
status = "pending"
[[milestones.tasks]]
id = "a"
title = "A"
status = "pending"
[[milestones.tasks]]
id = "b"
title = "B"
status = "pending"
depends_on = ["a"]
"#,
        )
        .unwrap()
    }

    #[test]
    fn ledger_status_wins_and_dependencies_block_until_done() {
        let run = RunId::new();
        let ledger: Ledger = [("a".to_string(), (RoadmapStatus::Running, false))].into();
        let tasks = assemble("idea", run, Some(&roadmap()), &ledger, &[], false);
        assert_eq!(tasks[0].status, RoadmapStatus::Running);
        assert_eq!(tasks[1].blocked_by, ["a"]);

        let ledger: Ledger = [("a".to_string(), (RoadmapStatus::Completed, true))].into();
        let tasks = assemble("idea", run, Some(&roadmap()), &ledger, &[], false);
        assert!(tasks[0].verified);
        assert!(tasks[1].blocked_by.is_empty());
    }

    #[test]
    fn discovered_tasks_join_their_origin_milestone() {
        let run = RunId::new();
        let discovered = vec![("c".to_string(), "Fix flaky timer".to_string(), "a".to_string())];
        let tasks = assemble("idea", run, Some(&roadmap()), &Ledger::new(), &discovered, true);
        let c = tasks.iter().find(|t| t.id == "c").unwrap();
        assert_eq!(c.discovered_from.as_deref(), Some("a"));
        assert_eq!(c.milestone.as_deref(), Some("Core"));
        assert_eq!(c.status, RoadmapStatus::Pending);
        assert!(c.mission_ended);
    }
}
