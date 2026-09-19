//! `surge task` — inspect and control the project task queue (ADR-0020).
//!
//! The queue's planning truth is `.surge/roadmap.toml`; this command writes
//! both the file and the mirrored registry row, so a later mirror pass does
//! not undo the change. Execution controls (`pause`/`resume`/`requeue`) act
//! on the registry row only — they are facts the file cannot express.

use anyhow::{Context, Result, anyhow, bail};
use clap::{Args, Subcommand};
use surge_core::{Priority, RoadmapArtifact};
use surge_orchestrator::scheduler::{QueueEntry, QueuePolicy};
use surge_persistence::runs::Storage;
use surge_persistence::task_queue::{DispatchState, TaskQueueFilter, TaskQueueRow};

use crate::commands::common::{project_root, surge_home_dir};

/// Relative path of the project roadmap inside the repository.
const ROADMAP_RELPATH: &str = ".surge/roadmap.toml";

/// `surge task` subcommands.
#[derive(Debug, Clone, Subcommand)]
pub enum TaskCommands {
    /// List the queue: state, priority, dependencies and readiness.
    List(TaskListArgs),
    /// Set a task's manual priority (writes the roadmap and the queue row).
    Priority(TaskPriorityArgs),
    /// Pause a task: it leaves the ready set until resumed.
    Pause(TaskRefArgs),
    /// Resume a paused task.
    Resume(TaskRefArgs),
    /// Return a failed task to the queue (the dependency-unblock action).
    Requeue(TaskRefArgs),
    /// Skip a task: it stops blocking its dependents.
    Skip(TaskRefArgs),
}

/// Arguments for `surge task list`.
#[derive(Debug, Clone, Args)]
pub struct TaskListArgs {
    /// Emit JSON instead of a table.
    #[arg(long)]
    pub json: bool,
    /// Show the derived ready set even when empty.
    #[arg(long)]
    pub ready: bool,
}

/// Arguments for `surge task priority`.
#[derive(Debug, Clone, Args)]
pub struct TaskPriorityArgs {
    /// Task id (`m1-t1`).
    pub task_id: String,
    /// New priority: `low`, `medium`, `high` or `critical`.
    pub priority: String,
}

/// Arguments naming one task.
#[derive(Debug, Clone, Args)]
pub struct TaskRefArgs {
    /// Task id (`m1-t1`).
    pub task_id: String,
}

/// Run `surge task`.
///
/// # Errors
/// Storage or roadmap I/O failures; an unknown task id.
pub async fn run(command: TaskCommands) -> Result<()> {
    let cwd = std::env::current_dir().context("resolve current directory")?;
    let root = project_root(&cwd);
    match command {
        TaskCommands::List(args) => list(&root, args).await,
        TaskCommands::Priority(args) => set_priority(&root, args).await,
        TaskCommands::Pause(args) => set_task_paused(&root, &args.task_id, true).await,
        TaskCommands::Resume(args) => set_task_paused(&root, &args.task_id, false).await,
        TaskCommands::Requeue(args) => requeue(&root, &args.task_id).await,
        TaskCommands::Skip(args) => skip(&root, &args.task_id).await,
    }
}

async fn open_storage() -> Result<std::sync::Arc<Storage>> {
    Storage::open(&surge_home_dir()?)
        .await
        .context("open storage")
}

/// Read the project's roadmap and mirror it into the queue.
///
/// Every mutating `surge task` command starts here so the row it edits is
/// current with the file.
async fn mirror(root: &std::path::Path) -> Result<()> {
    let roadmap_path = root.join(ROADMAP_RELPATH);
    let text = std::fs::read_to_string(&roadmap_path)
        .with_context(|| format!("read {}", roadmap_path.display()))?;
    let roadmap: RoadmapArtifact =
        toml::from_str(&text).with_context(|| format!("parse {}", roadmap_path.display()))?;
    let hash = surge_core::ContentHash::compute(text.as_bytes());
    let storage = open_storage().await?;
    let queue = storage.task_queue_store();
    queue
        .register_project(root, now_ms())
        .context("register project")?;
    queue
        .mirror(root, &hash, &roadmap.to_queue_entries(), now_ms())
        .context("mirror roadmap into the queue")?;
    Ok(())
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

async fn list(root: &std::path::Path, args: TaskListArgs) -> Result<()> {
    mirror(root).await?;
    let storage = open_storage().await?;
    let queue = storage.task_queue_store();
    let rows = queue
        .list(&TaskQueueFilter {
            project_root: Some(root.to_path_buf()),
            ..Default::default()
        })
        .context("list queue")?;
    let dep_states = queue.dependency_states(root).context("dependency states")?;
    let config = surge_core::SurgeConfig::load_or_default().unwrap_or_default();
    let entries: Vec<QueueEntry> = rows
        .iter()
        .map(|row| QueueEntry {
            task_id: row.task_id.clone(),
            priority: row.priority,
            depends_on: row.depends_on.clone(),
            dep_states: dep_states.get(&row.task_id).cloned().unwrap_or_default(),
            size: row.size,
            enqueued_at: row.enqueued_at,
            skipped_dispatches: row.skipped_dispatches,
        })
        .collect();
    let decision = QueuePolicy::next(&entries, &config.queue);

    if args.json {
        let payload = serde_json::json!({
            "tasks": rows,
            "ready": decision.ready,
            "blocked_by_failed": decision.blocked_by_failed.iter().map(|b| {
                serde_json::json!({
                    "task_id": b.task_id,
                    "by": b.by.iter().map(|(id, state)| serde_json::json!({
                        "task_id": id,
                        "state": state,
                    })).collect::<Vec<_>>(),
                })
            }).collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
        return Ok(());
    }

    print_task_table(&mut std::io::stdout().lock(), &rows, &decision);
    Ok(())
}

fn print_task_table(
    out: &mut impl std::io::Write,
    rows: &[TaskQueueRow],
    decision: &surge_orchestrator::scheduler::QueueDecision,
) {
    if rows.is_empty() {
        let _ = writeln!(out, "Queue is empty (no task rows for this project).");
        return;
    }
    let _ = writeln!(
        out,
        "{:<20} {:<10} {:<11} {:<9} {:<12} RUN",
        "TASK", "PRIORITY", "STATE", "ATTEMPT", "DEPENDS_ON"
    );
    for row in rows {
        let _ = writeln!(
            out,
            "{:<20} {:<10} {:<11} {:<9} {:<12} {}",
            row.task_id,
            row.priority,
            row.dispatch_state.as_str(),
            row.attempt,
            if row.depends_on.is_empty() {
                "-".to_string()
            } else {
                row.depends_on.join(",")
            },
            row.run_id
                .map(|id| id.to_string())
                .unwrap_or_else(|| "-".into()),
        );
    }
    let _ = writeln!(out, "\n{} task(s).", rows.len());
    if !decision.ready.is_empty() {
        let _ = writeln!(out, "Ready: {}", decision.ready.join(", "));
    }
    for blocked in &decision.blocked_by_failed {
        let by = blocked
            .by
            .iter()
            .map(|(id, state)| format!("{id} ({state})"))
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(
            out,
            "blocked_by_failed: {} — blocked by {}",
            blocked.task_id, by
        );
    }
}

async fn set_priority(root: &std::path::Path, args: TaskPriorityArgs) -> Result<()> {
    let priority: Priority = args.priority.parse().map_err(|e| anyhow!("{e}"))?;
    let roadmap_path = root.join(ROADMAP_RELPATH);
    let text = std::fs::read_to_string(&roadmap_path)
        .with_context(|| format!("read {}", roadmap_path.display()))?;
    let mut roadmap: RoadmapArtifact =
        toml::from_str(&text).with_context(|| format!("parse {}", roadmap_path.display()))?;
    let Some(task) = roadmap
        .milestones
        .iter_mut()
        .flat_map(|m| m.tasks.iter_mut())
        .find(|t| t.id == args.task_id)
    else {
        bail!(
            "task {:?} is not in {}",
            args.task_id,
            roadmap_path.display()
        );
    };
    task.priority = priority;
    let rendered = toml::to_string_pretty(&roadmap).context("render roadmap")?;
    std::fs::write(&roadmap_path, &rendered)
        .with_context(|| format!("write {}", roadmap_path.display()))?;

    // Mirror and force the row's priority, so a dispatch that races the
    // file write still sees the new order.
    mirror(root).await?;
    let storage = open_storage().await?;
    storage
        .task_queue_store()
        .set_priority(root, &args.task_id, priority, now_ms())
        .context("set queue priority")?;
    println!("{} → {priority}", args.task_id);
    Ok(())
}

async fn set_task_paused(root: &std::path::Path, task_id: &str, paused: bool) -> Result<()> {
    mirror(root).await?;
    let storage = open_storage().await?;
    let queue = storage.task_queue_store();
    let Some(row) = queue.get(root, task_id).context("get task")? else {
        bail!("task {task_id:?} is not in the queue (is the roadmap mirrored?)");
    };
    if paused {
        if row.dispatch_state == DispatchState::Dispatched {
            bail!(
                "task {task_id:?} is currently running (run {}); `surge task pause` affects \
                 queued tasks only — use `surge engine stop` for a running one",
                row.run_id.map(|id| id.to_string()).unwrap_or_default()
            );
        }
        queue
            .set_task_paused(root, task_id, true, now_ms())
            .context("pause task")?;
        println!("paused {task_id}");
    } else {
        queue
            .set_task_paused(root, task_id, false, now_ms())
            .context("resume task")?;
        println!("resumed {task_id}");
    }
    Ok(())
}

async fn requeue(root: &std::path::Path, task_id: &str) -> Result<()> {
    mirror(root).await?;
    let storage = open_storage().await?;
    let queue = storage.task_queue_store();
    let Some(row) = queue.get(root, task_id).context("get task")? else {
        bail!("task {task_id:?} is not in the queue");
    };
    if row.dispatch_state == DispatchState::Dispatched {
        bail!("task {task_id:?} is currently running; requeue applies to settled tasks");
    }
    if !queue.requeue(root, task_id, now_ms()).context("requeue")? {
        bail!(
            "task {task_id:?} is not in a requeueable state ({})",
            row.dispatch_state.as_str()
        );
    }
    println!("requeued {task_id}");
    Ok(())
}

async fn skip(root: &std::path::Path, task_id: &str) -> Result<()> {
    mirror(root).await?;
    let storage = open_storage().await?;
    let queue = storage.task_queue_store();
    let Some(_row) = queue.get(root, task_id).context("get task")? else {
        bail!("task {task_id:?} is not in the queue");
    };
    queue.skip(root, task_id, now_ms()).context("skip task")?;
    println!("skipped {task_id}");
    Ok(())
}
