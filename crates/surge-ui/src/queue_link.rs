//! Bridge from the UI to the daemon's project task queue
//! (`surge_persistence::task_queue`).
//!
//! `.surge/roadmap.toml` is the planning truth (same as the daemon
//! scheduler and `surge task` CLI); the registry `task_queue` mirrors
//! it. The UI is a read-only observer of that queue: on demand it
//! re-mirrors the project's roadmap (exactly what every `surge task`
//! command does first) and reads the rows back for the Backlog board.
//!
//! The registry SQLite lives at `~/.surge/`; opening it needs a tokio
//! multi-thread runtime — the app keeps one in `main.rs`, so reads run
//! through `cx.background_executor().spawn` from a sync render path.
//!
//! `project_root` must be spelled exactly as the queue's partition key
//! does it: canonicalized (the same normalization `surge task` applies,
//! see `surge-cli::commands::common::normalize_project_root`).

use std::path::PathBuf;

use surge_persistence::runs::Storage;
use surge_persistence::task_queue::{TaskQueueFilter, TaskQueueRow};

/// The project's roadmap file, mirrored into the queue.
pub const ROADMAP_RELPATH: &str = ".surge/roadmap.toml";

/// One backlog card's worth of queue data, pre-mapped for the board.
#[derive(Debug, Clone)]
pub struct QueueCard {
    pub task_id: String,
    pub title: String,
    pub priority: String,
    pub depends_on: Vec<String>,
    pub blocked_by: Vec<String>,
    pub size: Option<String>,
    pub dispatch_state: String,
    pub skipped_dispatches: u32,
    pub attempt: u32,
    pub run_id: Option<surge_core::id::RunId>,
    pub enqueued_at: i64,
}

/// Canonicalize a project root the same way the queue partition key is
/// spelled everywhere else.
#[must_use]
pub fn normalize_project_root(root: &std::path::Path) -> PathBuf {
    root.canonicalize().unwrap_or_else(|_| root.to_path_buf())
}

/// Open the registry storage and re-mirror the project's roadmap into
/// the queue, then read every row for the project. One call per board
/// refresh: a single SQLite round-trip over a local file.
///
/// # Errors
/// Surface verbatim in the Backlog source pill — a failed mirror or
/// read must be visible, never silently empty.
pub async fn load_queue_cards(project_root: PathBuf) -> Result<Vec<QueueCard>, String> {
    let storage = Storage::open(surge_core::home::surge_home_dir().ok_or_else(|| {
        "could not resolve SURGE_HOME — the registry queue is unavailable".to_string()
    })?)
    .await
    .map_err(|e| format!("open registry: {e}"))?;
    let queue = storage.task_queue_store();

    mirror_roadmap(&queue, &project_root)?;

    let rows = queue
        .list(&TaskQueueFilter {
            project_root: Some(project_root.clone()),
            ..Default::default()
        })
        .map_err(|e| format!("list queue: {e}"))?;
    let dep_states = queue
        .dependency_states(&project_root)
        .map_err(|e| format!("dependency states: {e}"))?;

    Ok(rows
        .into_iter()
        .map(|row| queue_card(row, &dep_states))
        .collect())
}

/// Re-mirror `.surge/roadmap.toml` so the rows this board reads are
/// current with the file (same order of operations as `surge task`).
fn mirror_roadmap(
    queue: &surge_persistence::task_queue::TaskQueueStore,
    project_root: &std::path::Path,
) -> Result<(), String> {
    let roadmap_path = project_root.join(ROADMAP_RELPATH);
    let text = std::fs::read_to_string(&roadmap_path)
        .map_err(|e| format!("read {}: {e}", roadmap_path.display()))?;
    let roadmap: surge_core::RoadmapArtifact =
        toml::from_str(&text).map_err(|e| format!("parse {}: {e}", roadmap_path.display()))?;
    let hash = surge_core::ContentHash::compute(text.as_bytes());
    let now_ms = chrono::Utc::now().timestamp_millis();
    queue
        .register_project(project_root, now_ms)
        .map_err(|e| format!("register project: {e}"))?;
    queue
        .mirror(project_root, &hash, &roadmap.to_queue_entries(), now_ms)
        .map_err(|e| format!("mirror roadmap: {e}"))?;
    Ok(())
}

/// Map one queue row (+ dependency states) to a board card. Titles come
/// from the roadmap file when it is readable — the queue row itself
/// carries only ids.
fn queue_card(
    row: TaskQueueRow,
    dep_states: &std::collections::BTreeMap<
        String,
        std::collections::BTreeMap<String, surge_core::RoadmapStatus>,
    >,
) -> QueueCard {
    let blocked_by: Vec<String> = row
        .depends_on
        .iter()
        .filter(|dep| {
            !matches!(
                dep_states.get(&row.task_id).and_then(|deps| deps.get(*dep)),
                Some(surge_core::RoadmapStatus::Completed)
            )
        })
        .cloned()
        .collect();
    QueueCard {
        task_id: row.task_id.clone(),
        title: row.task_id,
        priority: row.priority.to_string(),
        depends_on: row.depends_on.clone(),
        blocked_by,
        size: row.size.map(|s| s.to_string()),
        dispatch_state: row.dispatch_state.as_str().to_string(),
        skipped_dispatches: row.skipped_dispatches,
        attempt: row.attempt,
        run_id: row.run_id,
        enqueued_at: row.enqueued_at,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_project_root_is_idempotent_for_a_real_dir() {
        let dir = std::env::temp_dir();
        let once = normalize_project_root(&dir);
        let twice = normalize_project_root(&once);
        assert_eq!(once, twice);
    }

    /// A card is blocked by every dependency that is not Completed —
    /// the same projection `QueuePolicy` reads readiness through.
    #[test]
    fn blocked_by_marks_pending_dependencies_only() {
        use surge_core::RoadmapStatus;
        let mut deps = std::collections::BTreeMap::new();
        deps.insert("m1-t1".to_string(), RoadmapStatus::Completed);
        deps.insert("m1-t2".to_string(), RoadmapStatus::Pending);
        deps.insert("m1-t3".to_string(), RoadmapStatus::Failed);
        let mut dep_states = std::collections::BTreeMap::new();
        dep_states.insert("m1-t4".to_string(), deps);

        let row_depends = vec![
            "m1-t1".to_string(),
            "m1-t2".to_string(),
            "m1-t3".to_string(),
        ];
        let blocked_by: Vec<String> = row_depends
            .iter()
            .filter(|dep| {
                !matches!(
                    dep_states.get("m1-t4").and_then(|d| d.get(*dep)),
                    Some(RoadmapStatus::Completed)
                )
            })
            .cloned()
            .collect();
        assert_eq!(blocked_by, vec!["m1-t2", "m1-t3"]);
    }
}
