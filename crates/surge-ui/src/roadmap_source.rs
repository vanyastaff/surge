//! Where the Roadmap screen's plan comes from, and how far along it is.
//!
//! A project's roadmap lives in one of two places:
//! 1. a `roadmap.toml` checked into the project (or `.surge/roadmap.toml`);
//! 2. the plan a bootstrap operation wrote for this project — the
//!    `roadmap_toml` artifact of its planning run.
//!
//! A plan alone says every task is pending. For (2) the implementation run's
//! task ledger is overlaid, so a task reads "completed · verified" only when
//! the engine recorded exactly that.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use surge_core::RunId;
use surge_core::content_hash::ContentHash;
use surge_core::roadmap::{RoadmapArtifact, RoadmapStatus, RoadmapTaskId};

/// Where the shown plan was read from.
#[derive(Debug, Clone, PartialEq)]
pub enum RoadmapOrigin {
    /// A roadmap file in the project.
    ProjectFile(PathBuf),
    /// The plan of a bootstrap operation for this project.
    Planned {
        planning_run: RunId,
        implementation_run: RunId,
        /// What the person asked for.
        prompt: String,
    },
}

/// A plan plus the live status of its tasks.
#[derive(Debug, Clone)]
pub struct LoadedRoadmap {
    pub artifact: RoadmapArtifact,
    pub origin: RoadmapOrigin,
}

/// `roadmap.toml` in the project root, then `.surge/roadmap.toml`.
pub fn project_file(root: &Path) -> Option<LoadedRoadmap> {
    for candidate in [root.join("roadmap.toml"), root.join(".surge/roadmap.toml")] {
        let Ok(text) = std::fs::read_to_string(&candidate) else {
            continue;
        };
        match toml::from_str::<RoadmapArtifact>(&text) {
            Ok(artifact) => {
                return Some(LoadedRoadmap {
                    artifact,
                    origin: RoadmapOrigin::ProjectFile(candidate),
                });
            },
            Err(error) => {
                tracing::warn!(path = %candidate.display(), %error, "roadmap.toml parse failed");
            },
        }
    }
    None
}

/// Read a named artifact of `run_id` straight from the content-addressed
/// store layout (`runs/<run>/artifacts/index.json` → `<hex>`).
pub fn read_run_artifact(surge_home: &Path, run_id: RunId, name: &str) -> Option<Vec<u8>> {
    let dir = surge_home
        .join("runs")
        .join(run_id.to_string())
        .join("artifacts");
    let index: HashMap<String, String> =
        serde_json::from_slice(&std::fs::read(dir.join("index.json")).ok()?).ok()?;
    let hash: ContentHash = index.get(name)?.parse().ok()?;
    let bytes = std::fs::read(dir.join(hash.to_hex())).ok()?;
    // The store is content-addressed; a mismatch means a torn or foreign file.
    (ContentHash::compute(&bytes) == hash).then_some(bytes)
}

/// Overlay ledger rows (`task_id → (status, verified)`) onto the plan.
/// Tasks the ledger never touched keep their planned status.
pub fn apply_ledger(
    artifact: &mut RoadmapArtifact,
    ledger: &HashMap<RoadmapTaskId, (RoadmapStatus, bool)>,
) {
    for milestone in &mut artifact.milestones {
        for task in &mut milestone.tasks {
            if let Some(&(status, verified)) = ledger.get(&task.id) {
                task.status = status;
                task.verified = verified;
            }
        }
        milestone.status = rollup(milestone.tasks.iter().map(|t| t.status), milestone.status);
    }
}

/// A milestone's status from its tasks: failed if any failed, running if
/// any is in flight, completed when all are done; otherwise as planned.
fn rollup(tasks: impl Iterator<Item = RoadmapStatus>, planned: RoadmapStatus) -> RoadmapStatus {
    let statuses: Vec<RoadmapStatus> = tasks.collect();
    if statuses.is_empty() {
        return planned;
    }
    let any = |s: RoadmapStatus| statuses.contains(&s);
    if any(RoadmapStatus::Failed) || any(RoadmapStatus::FailedVerification) {
        RoadmapStatus::Failed
    } else if any(RoadmapStatus::Running) || any(RoadmapStatus::ReadyForVerification) {
        RoadmapStatus::Running
    } else if statuses
        .iter()
        .all(|s| matches!(s, RoadmapStatus::Completed | RoadmapStatus::Skipped))
    {
        RoadmapStatus::Completed
    } else if any(RoadmapStatus::Completed) {
        RoadmapStatus::Running
    } else {
        planned
    }
}

/// A bootstrap operation of the project: the idea, its planning run and
/// the implementation run it launches.
#[derive(Debug, Clone)]
pub struct Operation {
    pub planning_run: RunId,
    pub implementation_run: RunId,
    pub prompt: String,
}

/// The project's bootstrap operations, newest first.
pub async fn operations(project_root: &Path, surge_home: &Path) -> Vec<Operation> {
    use surge_persistence::runs::bootstrap_operations::BootstrapStoredPayload;

    let Ok(storage) = surge_persistence::runs::Storage::open(surge_home).await else {
        return Vec::new();
    };
    let root = std::fs::canonicalize(project_root).unwrap_or_else(|_| project_root.to_path_buf());
    let Ok(records) = storage.bootstrap_operation_store().list() else {
        return Vec::new();
    };
    let mut found: Vec<(u64, Operation)> = records
        .into_iter()
        .filter_map(|record| match &record.payload {
            BootstrapStoredPayload::V1 { intent, .. } => {
                let path = intent.project_path();
                let same =
                    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()) == root;
                same.then(|| {
                    (
                        record.status.queue_sequence,
                        Operation {
                            planning_run: record.status.planning_run,
                            implementation_run: record.status.implementation_run,
                            prompt: intent.prompt().to_string(),
                        },
                    )
                })
            },
            BootstrapStoredPayload::Unsupported { .. } => None,
        })
        .collect();
    found.sort_by_key(|(sequence, _)| std::cmp::Reverse(*sequence));
    found.into_iter().map(|(_, op)| op).collect()
}

/// The newest bootstrap plan for `project_root`, with its ledger applied.
pub async fn planned(project_root: &Path, surge_home: &Path) -> Option<LoadedRoadmap> {
    use surge_persistence::task_ledger::TaskLedgerIndexFilter;

    for op in operations(project_root, surge_home).await {
        let Some(bytes) = read_run_artifact(surge_home, op.planning_run, "roadmap_toml") else {
            continue;
        };
        let Ok(text) = String::from_utf8(bytes) else {
            continue;
        };
        let Ok(mut artifact) = toml::from_str::<RoadmapArtifact>(&text) else {
            tracing::warn!(run = %op.planning_run, "planned roadmap did not parse");
            continue;
        };
        let ledger: HashMap<RoadmapTaskId, (RoadmapStatus, bool)> =
            match surge_persistence::runs::Storage::open(surge_home).await {
                Ok(storage) => storage
                    .task_ledger_store()
                    .list(&TaskLedgerIndexFilter {
                        status: None,
                        project_path: None,
                        run_id: Some(op.implementation_run),
                        discovered_only: false,
                        limit: None,
                    })
                    .unwrap_or_default()
                    .into_iter()
                    .map(|row| (row.task_id.into(), (row.status, row.verified)))
                    .collect(),
                Err(_) => HashMap::new(),
            };
        apply_ledger(&mut artifact, &ledger);
        return Some(LoadedRoadmap {
            artifact,
            origin: RoadmapOrigin::Planned {
                planning_run: op.planning_run,
                implementation_run: op.implementation_run,
                prompt: op.prompt,
            },
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{apply_ledger, read_run_artifact, rollup};
    use std::collections::HashMap;
    use surge_core::RunId;
    use surge_core::content_hash::ContentHash;
    use surge_core::roadmap::{RoadmapArtifact, RoadmapStatus};

    const PLAN: &str = r#"
schema_version = 2
[[milestones]]
id = "m1"
title = "Timer core"
status = "pending"
[[milestones.tasks]]
id = "t1"
title = "Countdown"
status = "pending"
[[milestones.tasks]]
id = "t2"
title = "Controls"
status = "pending"
"#;

    fn plan() -> RoadmapArtifact {
        toml::from_str(PLAN).expect("fixture plan parses")
    }

    #[test]
    fn ledger_overrides_planned_status_and_verification() {
        let mut artifact = plan();
        let ledger = HashMap::from([("t1".into(), (RoadmapStatus::Completed, true))]);
        apply_ledger(&mut artifact, &ledger);
        let tasks = &artifact.milestones[0].tasks;
        assert_eq!(tasks[0].status, RoadmapStatus::Completed);
        assert!(tasks[0].verified);
        // Untouched by the ledger: stays as planned, never promoted.
        assert_eq!(tasks[1].status, RoadmapStatus::Pending);
        assert!(!tasks[1].verified);
        // One of two done → the milestone is in progress, not complete.
        assert_eq!(artifact.milestones[0].status, RoadmapStatus::Running);
    }

    #[test]
    fn rollup_failure_wins_and_all_done_completes() {
        use RoadmapStatus::*;
        assert_eq!(rollup([Completed, Failed].into_iter(), Pending), Failed);
        assert_eq!(rollup([Completed, Skipped].into_iter(), Pending), Completed);
        assert_eq!(rollup([Pending, Pending].into_iter(), Paused), Paused);
        assert_eq!(rollup(std::iter::empty(), Pending), Pending);
    }

    #[test]
    fn run_artifact_is_read_through_its_index_and_hash_checked() {
        let home = tempfile::tempdir().unwrap();
        let run = RunId::new();
        let dir = home
            .path()
            .join("runs")
            .join(run.to_string())
            .join("artifacts");
        std::fs::create_dir_all(&dir).unwrap();
        let hash = ContentHash::compute(PLAN.as_bytes());
        std::fs::write(dir.join(hash.to_hex()), PLAN).unwrap();
        std::fs::write(
            dir.join("index.json"),
            serde_json::json!({ "roadmap_toml": hash.to_string() }).to_string(),
        )
        .unwrap();
        assert_eq!(
            read_run_artifact(home.path(), run, "roadmap_toml").as_deref(),
            Some(PLAN.as_bytes())
        );
        assert!(read_run_artifact(home.path(), run, "missing").is_none());

        // A file whose bytes no longer match its address is not trusted.
        std::fs::write(dir.join(hash.to_hex()), "tampered").unwrap();
        assert!(read_run_artifact(home.path(), run, "roadmap_toml").is_none());
    }
}
