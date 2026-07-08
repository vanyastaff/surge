//! `surge inbox` — the fleet inbox: every run grouped by what it needs from
//! the operator right now (Needs input / Working / Done), blocked-first
//! (Phase 2 B1).
//!
//! There is no persisted "blocked on human" flag today, so attention is derived
//! authoritatively by folding each non-terminal run's event log into a
//! `RunState` and classifying it. Terminal runs are read cheaply from the
//! registry. For a handful of active runs this is fine; a registry-level
//! attention index (making this a single query) is a documented follow-up.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Args;
use serde::Serialize;
use surge_core::{Attention, RunState, TerminalReason};
use surge_persistence::runs::Storage;
use surge_persistence::runs::registry::{RunFilter, RunSummary};

use crate::commands::common::surge_home_dir;
use crate::commands::run_fold::fold_run_state;

/// Arguments for `surge inbox`.
#[derive(Args, Debug)]
pub struct InboxArgs {
    /// Accepted for compatibility; project scoping is currently always on
    /// (runs store their worktree path, not the origin repo), so this is a
    /// no-op today.
    #[arg(long)]
    pub all_projects: bool,
    /// Also list the Done group in full (default: just a count).
    #[arg(long)]
    pub all: bool,
    /// Maximum runs to consider.
    #[arg(long, default_value_t = 200)]
    pub limit: usize,
    /// Emit JSON instead of grouped tables.
    #[arg(long)]
    pub json: bool,
}

/// One classified run for the inbox.
#[derive(Debug, Serialize)]
struct InboxEntry {
    run_id: String,
    project_path: PathBuf,
    /// `needs_input` | `working` | `done`.
    attention: &'static str,
    /// Terminal reason when done (`completed` / `failed` / `aborted`), else null.
    #[serde(skip_serializing_if = "Option::is_none")]
    done_reason: Option<&'static str>,
    /// Active node for a working/blocked run, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    active_node: Option<String>,
    /// Prompt the operator must answer, when blocked with one.
    #[serde(skip_serializing_if = "Option::is_none")]
    prompt: Option<String>,
    started_at_ms: i64,
}

/// Run `surge inbox`.
///
/// # Errors
/// Returns an error if storage cannot be opened or a run cannot be read.
pub async fn run(args: InboxArgs) -> Result<()> {
    let storage = Storage::open(&surge_home_dir()?)
        .await
        .context("open storage")?;
    // Per-project scoping is disabled for now: a run records its isolated
    // worktree path as project_path, which never equals the invoking repo, so a
    // current-dir filter silently matched nothing. Show all projects until runs
    // record their origin repo (tracked follow-up). `--all-projects` is kept as
    // an accepted no-op so scripts don't break.
    let _ = args.all_projects;
    let entries = collect_entries(&storage, None, args.limit).await?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&entries)?);
    } else {
        print_inbox(&entries, args.all);
    }
    Ok(())
}

/// List runs for `project_path` (or all) and classify each. Extracted from
/// [`run`] so the classification is testable against a live storage.
async fn collect_entries(
    storage: &std::sync::Arc<Storage>,
    project_path: Option<PathBuf>,
    limit: usize,
) -> Result<Vec<InboxEntry>> {
    // Fetch ALL runs, not a newest-`limit` window: a run still blocked on human
    // input can be older than `limit` more-recently-started (settled) runs, and
    // it must never fall out of the NEEDS INPUT group — the whole point of the
    // inbox. Classifying a terminal run is cheap (registry status, no fold), so
    // scanning all runs is fine; only non-terminal runs are folded.
    let summaries = storage
        .list_runs(RunFilter {
            status: None,
            project_path,
            limit: None,
        })
        .await
        .context("list runs")?;
    let mut entries = Vec::with_capacity(summaries.len());
    for summary in &summaries {
        entries.push(classify(storage, summary).await?);
    }
    // Bound only the settled/Done tail: keep every NEEDS INPUT / WORKING entry,
    // cap Done at `limit` (Done is a count/`--all` view anyway).
    entries.sort_by_key(|e| e.attention == "done");
    let done_start = entries
        .iter()
        .position(|e| e.attention == "done")
        .unwrap_or(entries.len());
    entries.truncate(done_start.saturating_add(limit));
    Ok(entries)
}

/// Classify one run. Terminal registry status short-circuits the fold; any
/// other run is folded for the authoritative Needs-input/Working split.
async fn classify(storage: &std::sync::Arc<Storage>, summary: &RunSummary) -> Result<InboxEntry> {
    let base = |attention: &'static str, done_reason, active_node, prompt| InboxEntry {
        run_id: summary.id.to_string(),
        project_path: summary.project_path.clone(),
        attention,
        done_reason,
        active_node,
        prompt,
        started_at_ms: summary.started_at_ms,
    };

    if summary.status.is_terminal() {
        return Ok(base(
            "done",
            Some(terminal_label(summary.status)),
            None,
            None,
        ));
    }

    // Non-terminal: fold the event log for the authoritative attention state.
    let reader = storage
        .open_run_reader(summary.id)
        .await
        .with_context(|| format!("open run {}", summary.id))?;
    let state = fold_run_state(&reader, summary.id).await?;
    let active_node = active_node(&state);
    Ok(match state.attention() {
        Attention::NeedsInput => base(
            "needs_input",
            None,
            active_node,
            state.pending_prompt().map(ToOwned::to_owned),
        ),
        Attention::Working => base("working", None, active_node, None),
        Attention::Done(reason) => base("done", Some(reason_label(reason)), None, None),
    })
}

fn active_node(state: &RunState) -> Option<String> {
    match state {
        RunState::Pipeline { cursor, .. } => Some(cursor.node.to_string()),
        _ => None,
    }
}

fn terminal_label(status: surge_core::RunStatus) -> &'static str {
    use surge_core::RunStatus;
    match status {
        RunStatus::Completed => "completed",
        RunStatus::Failed => "failed",
        RunStatus::Aborted => "aborted",
        RunStatus::Crashed => "crashed",
        RunStatus::Bootstrapping | RunStatus::Running => "running",
    }
}

fn reason_label(reason: TerminalReason) -> &'static str {
    match reason {
        TerminalReason::Completed => "completed",
        TerminalReason::Failed => "failed",
        TerminalReason::Aborted => "aborted",
    }
}

fn print_inbox(entries: &[InboxEntry], show_done: bool) {
    let needs: Vec<&InboxEntry> = entries
        .iter()
        .filter(|e| e.attention == "needs_input")
        .collect();
    let working: Vec<&InboxEntry> = entries
        .iter()
        .filter(|e| e.attention == "working")
        .collect();
    let done: Vec<&InboxEntry> = entries.iter().filter(|e| e.attention == "done").collect();

    // Blocked-first: the "needs me right now" group leads.
    println!("⚑ NEEDS INPUT ({})", needs.len());
    if needs.is_empty() {
        println!("  (nothing waiting on you)");
    } else {
        for e in &needs {
            let node = e.active_node.as_deref().unwrap_or("-");
            println!("  {}  @{}", short_run(&e.run_id), node);
            if let Some(prompt) = &e.prompt {
                println!("      ↳ {}", first_line(prompt));
            }
        }
    }

    println!("\n▶ WORKING ({})", working.len());
    for e in &working {
        let node = e.active_node.as_deref().unwrap_or("-");
        println!("  {}  @{}", short_run(&e.run_id), node);
    }

    if show_done {
        println!("\n✔ DONE ({})", done.len());
        for e in &done {
            println!(
                "  {}  {}",
                short_run(&e.run_id),
                e.done_reason.unwrap_or("done")
            );
        }
    } else {
        println!("\n✔ DONE: {} (use --all to list)", done.len());
    }
}

fn short_run(run_id: &str) -> &str {
    // ULID run ids are 26 chars; show the last 8 for a compact, still-unique
    // handle in a single-user local context.
    if run_id.len() > 8 {
        &run_id[run_id.len() - 8..]
    } else {
        run_id
    }
}

fn first_line(s: &str) -> &str {
    s.lines().next().unwrap_or(s).trim()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use surge_core::approvals::ApprovalPolicy;
    use surge_core::content_hash::ContentHash;
    use surge_core::graph::{Graph, GraphMetadata, SCHEMA_VERSION};
    use surge_core::keys::NodeKey;
    use surge_core::node::{Node, NodeConfig, Position};
    use surge_core::run_event::{EventPayload, RunConfig, VersionedEventPayload};
    use surge_core::sandbox::SandboxMode;
    use surge_core::terminal_config::{TerminalConfig, TerminalKind};
    use surge_core::{RunId, RunStatus};

    fn minimal_graph() -> Graph {
        let end = NodeKey::try_from("plan").unwrap();
        let mut nodes = BTreeMap::new();
        nodes.insert(
            end.clone(),
            Node {
                id: end.clone(),
                position: Position::default(),
                declared_outcomes: vec![],
                config: NodeConfig::Terminal(TerminalConfig {
                    kind: TerminalKind::Success,
                    message: None,
                }),
            },
        );
        Graph {
            schema_version: SCHEMA_VERSION,
            metadata: GraphMetadata {
                name: "inbox-test".into(),
                description: None,
                template_origin: None,
                created_at: chrono::Utc::now(),
                author: None,
                archetype: None,
            },
            start: end,
            nodes,
            edges: vec![],
            subgraphs: BTreeMap::new(),
        }
    }

    fn run_started() -> EventPayload {
        EventPayload::RunStarted {
            pipeline_template: None,
            project_path: PathBuf::from("/proj"),
            initial_prompt: "x".into(),
            config: RunConfig {
                sandbox_default: SandboxMode::WorkspaceWrite,
                approval_default: ApprovalPolicy::OnRequest,
                auto_pr: false,
                mcp_servers: Vec::new(),
            },
        }
    }

    fn pipeline_materialized() -> EventPayload {
        EventPayload::PipelineMaterialized {
            graph: Box::new(minimal_graph()),
            graph_hash: ContentHash::compute(b"g"),
        }
    }

    async fn append(writer: &surge_persistence::runs::RunWriter, payloads: Vec<EventPayload>) {
        for p in payloads {
            writer
                .append_event(VersionedEventPayload::new(p))
                .await
                .unwrap();
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn inbox_groups_runs_by_attention() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("proj");
        let storage = Storage::open(dir.path()).await.unwrap();

        // Blocked run: RunStarted → PipelineMaterialized → HumanInputRequested.
        let blocked = RunId::new();
        let w = storage.create_run(blocked, &project, None).await.unwrap();
        append(
            &w,
            vec![
                run_started(),
                pipeline_materialized(),
                EventPayload::HumanInputRequested {
                    node: NodeKey::try_from("plan").unwrap(),
                    session: None,
                    call_id: Some("c1".into()),
                    prompt: "Approve the plan?".into(),
                    schema: None,
                },
            ],
        )
        .await;
        w.flush().await.unwrap();

        // Working run: RunStarted → PipelineMaterialized (no gate).
        let working = RunId::new();
        let w2 = storage.create_run(working, &project, None).await.unwrap();
        append(&w2, vec![run_started(), pipeline_materialized()]).await;
        w2.flush().await.unwrap();

        // Done run: registry status forced terminal (short-circuits the fold).
        let done = RunId::new();
        let w3 = storage.create_run(done, &project, None).await.unwrap();
        append(&w3, vec![run_started()]).await;
        w3.flush().await.unwrap();
        storage
            .set_run_status(&done, RunStatus::Completed, Some(1))
            .await
            .unwrap();

        let entries = collect_entries(&storage, Some(project.clone()), 100)
            .await
            .unwrap();
        assert_eq!(entries.len(), 3);

        let find = |id: RunId| {
            entries
                .iter()
                .find(|e| e.run_id == id.to_string())
                .unwrap_or_else(|| panic!("missing {id}"))
        };
        let b = find(blocked);
        assert_eq!(b.attention, "needs_input");
        assert_eq!(b.prompt.as_deref(), Some("Approve the plan?"));
        assert_eq!(b.active_node.as_deref(), Some("plan"));

        assert_eq!(find(working).attention, "working");

        let d = find(done);
        assert_eq!(d.attention, "done");
        assert_eq!(d.done_reason, Some("completed"));
    }
}
