//! `surge inbox` — the fleet inbox: every run grouped by what it needs from
//! the operator right now (Needs input / Working / Done), blocked-first
//! (Phase 2 B1).
//!
//! There is no persisted "blocked on human" flag today, so attention is derived
//! authoritatively by folding each non-terminal run's event log into a
//! `RunState` and classifying it. Attention for a terminal run is read cheaply
//! from the registry status alone (no fold). The R34–R36 capacity scan is a
//! separate question with its own, different cost: it reads a run's full
//! event log (`read_events(0..MAX)`), for every non-terminal run (a second
//! full read alongside the fold above) and every terminal `Failed`/
//! `Aborted`/`Crashed` run (`Completed` is the only terminal class skipped).
//! Accepted perf debt for a handful of active runs; not free at fleet scale,
//! and not merged into the fold's own read yet — a documented follow-up.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Args;
use serde::Serialize;
use surge_core::capacity::CapacityStatus;
use surge_core::run_event::EventPayload;
use surge_core::{Attention, RunId, RunState, RunStatus, TerminalReason};
use surge_persistence::runs::RunReader;
use surge_persistence::runs::Storage;
use surge_persistence::runs::registry::{RunFilter, RunSummary};
use surge_persistence::runs::seq::EventSeq;

use crate::commands::common::surge_home_dir;
use crate::commands::run_fold::fold_run_state;

/// Arguments for `surge inbox`.
#[derive(Args, Debug)]
pub struct InboxArgs {
    /// Accepted for compatibility. Per-project scoping is currently disabled —
    /// all projects are always shown (runs store their worktree path, not the
    /// origin repo), so this flag is a no-op today.
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
    /// Best-effort rate-limit capacity signal (R34–R36) read from the run's
    /// own event log — a `StageFailed` reason, attributed to the agent from
    /// the preceding `SessionOpened`. `NeverObserved` is the common case
    /// (R35.1); `Unclassified` (a real failure that didn't match a known
    /// rate-limit shape) is kept distinct from it rather than folded into
    /// the same silence — see `CapacityStatus`'s doc.
    #[serde(skip_serializing_if = "CapacityStatus::is_never_observed")]
    capacity: CapacityStatus,
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
    // inbox. This is not free — see the module doc's note on `classify`'s own
    // cost (a full `read_events(0..MAX)` per non-terminal run, and per terminal
    // `Failed`/`Aborted`/`Crashed` run too) — but that cost is accepted debt for
    // a handful of active runs, not a reason to risk dropping one from NEEDS
    // INPUT by pre-truncating before classification.
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

/// Classify one run. Terminal registry status short-circuits the *fold*
/// (state derivation is cheap from the registry alone); the capacity scan is
/// a separate question with a different cheap class, decided per
/// `RunStatus` below — one run's own construction, not this file's fold
/// short-circuit, is what tells you whether a capacity signal could live
/// there:
///
/// - `Failed`: a run that failed *from* a rate limit is, by construction,
///   `Failed` — this is the class the signal is most likely to be found in.
/// - `Aborted`: an operator can abort after watching repeated rate-limit
///   retries; the signal can equally sit here.
/// - `Crashed`: assigned by `Storage::list_runs` itself, rewriting a dead
///   daemon's `Running`/`Bootstrapping` run — a status about the *daemon*,
///   not the pipeline outcome, so a run that hit a 429 and then had its
///   daemon die carries the exact same `StageFailed` in its log and must be
///   scanned too, not skipped because the label differs from `Failed`.
/// - `Completed`: safe to skip — a run that reached its own terminal
///   success node necessarily passed through the `StageCompleted` this scan
///   already resets an in-progress signal on.
async fn classify(storage: &std::sync::Arc<Storage>, summary: &RunSummary) -> Result<InboxEntry> {
    let base = |attention: &'static str, done_reason, active_node, prompt, capacity| InboxEntry {
        run_id: summary.id.to_string(),
        project_path: summary.project_path.clone(),
        attention,
        done_reason,
        active_node,
        prompt,
        capacity,
        started_at_ms: summary.started_at_ms,
    };

    if summary.status.is_terminal() {
        let capacity = match summary.status {
            RunStatus::Failed | RunStatus::Aborted | RunStatus::Crashed => {
                scan_run_capacity(storage, summary.id).await
            },
            // `Bootstrapping`/`Running`/`Parked` cannot reach here
            // (`is_terminal()` gates this branch, and `Parked.is_terminal()`
            // is `false` — Task 12); kept explicit rather than a wildcard so
            // a future `RunStatus` variant forces a decision here instead of
            // silently defaulting.
            RunStatus::Completed
            | RunStatus::Bootstrapping
            | RunStatus::Running
            | RunStatus::Parked => CapacityStatus::NeverObserved,
        };
        return Ok(base(
            "done",
            Some(terminal_label(summary.status)),
            None,
            None,
            capacity,
        ));
    }

    // Non-terminal: fold the event log for the authoritative attention state.
    let reader = storage
        .open_run_reader(summary.id)
        .await
        .with_context(|| format!("open run {}", summary.id))?;
    let state = fold_run_state(&reader, summary.id).await?;
    let active_node = active_node(&state);
    let capacity = scan_capacity_signal(&reader, summary.id)
        .await
        .unwrap_or_else(|err| {
            tracing::debug!(run = %summary.id, error = %err, "capacity signal scan failed; showing none");
            CapacityStatus::NeverObserved
        });
    Ok(match state.attention() {
        Attention::NeedsInput => base(
            "needs_input",
            None,
            active_node,
            state.pending_prompt().map(ToOwned::to_owned),
            capacity,
        ),
        Attention::Working => base("working", None, active_node, None, capacity),
        // Minimal, compiling treatment only — Task 12 M1 adds the
        // `Attention::Waiting` variant so `RunState`/`Attention` compile
        // and fold correctly; a dedicated inbox group surfacing `until` is
        // M5's job (`.autopilot/competitive-waves/tickets/12-capacity-scheduling.md`).
        Attention::Waiting { .. } => base("waiting", None, active_node, None, capacity),
        Attention::Done(reason) => base("done", Some(reason_label(reason)), None, None, capacity),
    })
}

/// Open a fresh reader for a terminal run and scan it for a capacity
/// signal. Best-effort: a scan failure (or a run whose reader has already
/// been pruned) must never block the rest of the inbox listing over one
/// run's optional capacity line (R35.1) — it reads as `NeverObserved`, the
/// same as a run this scan legitimately found nothing in.
async fn scan_run_capacity(storage: &std::sync::Arc<Storage>, run_id: RunId) -> CapacityStatus {
    match storage.open_run_reader(run_id).await {
        Ok(reader) => scan_capacity_signal(&reader, run_id)
            .await
            .unwrap_or_else(|err| {
                tracing::debug!(run = %run_id, error = %err, "capacity signal scan failed; showing none");
                CapacityStatus::NeverObserved
            }),
        Err(err) => {
            tracing::debug!(run = %run_id, error = %err, "could not open reader for capacity scan; showing none");
            CapacityStatus::NeverObserved
        },
    }
}

/// Best-effort capacity signal for `run_id` (R34–R36): the classification of
/// each node's most recent `StageFailed` reason, attributed to the runtime
/// from the last `SessionOpened.agent_id` seen before it — **not** `.agent`,
/// which carries the node's flow-authored *profile* (e.g.
/// `"implementer@1.0"`), a role identity, not a runtime. Keying
/// `CapacityWindow.runtime` by profile would read one real runtime
/// configured under two profiles as two different runtimes. `agent_id` is
/// `None` for a run recorded before this field existed, or opened via the
/// no-profile-registry legacy path; a `StageFailed` with no known runtime
/// still means "saw something, cannot classify it", not silence — see the
/// `None` arm below.
///
/// Tracked **per node**, not as one run-wide slot: a flow node's profile
/// (and so its runtime) is fixed for the node's lifetime, so `node` alone
/// identifies "which runtime this status belongs to" without also tracking
/// per-node runtime history. Per-node tracking matters because a run is not
/// one long attempt at one node — `plan` failing with a 429 and `review`
/// separately failing with an unrelated error must not merge into a single
/// downgraded status, and `review` later completing must not erase `plan`'s
/// still-open signal (a single shared slot did both, incorrectly). A node's
/// entry clears (to absence, i.e. that node no longer contributes) on a
/// `StageCompleted` for that same node. The merged result across all nodes
/// prefers [`CapacityStatus::Known`] over [`CapacityStatus::Unclassified`]
/// over the all-clear [`CapacityStatus::NeverObserved`] — the most
/// informative live signal wins.
async fn scan_capacity_signal(reader: &RunReader, run_id: RunId) -> Result<CapacityStatus> {
    let events = reader
        .read_events(EventSeq(0)..EventSeq(u64::MAX))
        .await
        .with_context(|| format!("read events for {run_id}"))?;
    let mut current_runtime: Option<String> = None;
    let mut by_node: std::collections::HashMap<surge_core::keys::NodeKey, CapacityStatus> =
        std::collections::HashMap::new();
    for read in events {
        let observed_at =
            chrono::DateTime::from_timestamp_millis(read.timestamp_ms).unwrap_or_default();
        match read.payload.payload {
            EventPayload::SessionOpened { agent_id, .. } => current_runtime = agent_id,
            EventPayload::StageFailed { node, reason, .. } => {
                let status = match &current_runtime {
                    Some(runtime) => {
                        match surge_core::capacity::CapacityWindow::from_observed_error(
                            runtime.clone(),
                            &reason,
                            observed_at,
                        ) {
                            Some(window) => CapacityStatus::Known(window),
                            None => CapacityStatus::Unclassified,
                        }
                    },
                    // A failure with no known runtime still means "saw
                    // something, cannot classify it" — never silence.
                    None => CapacityStatus::Unclassified,
                };
                by_node.insert(node, status);
            },
            EventPayload::StageCompleted { node, .. } => {
                by_node.remove(&node);
            },
            _ => {},
        }
    }
    Ok(by_node
        .into_values()
        .max_by_key(capacity_status_rank)
        .unwrap_or(CapacityStatus::NeverObserved))
}

/// Priority for merging several nodes' [`CapacityStatus`] into one: a
/// concrete window outranks "saw something, can't classify it", which
/// outranks nothing. Ties (two nodes both `Known`) resolve to whichever
/// [`HashMap::into_values`] yields last — an accepted, undocumented-order
/// simplification for the rare case of two simultaneously open signals.
fn capacity_status_rank(status: &CapacityStatus) -> u8 {
    match status {
        CapacityStatus::NeverObserved => 0,
        CapacityStatus::Unclassified => 1,
        CapacityStatus::Known(_) => 2,
    }
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
        // Only ever called with `is_terminal()` statuses (see the call
        // site above); `Bootstrapping`/`Running`/`Parked` are unreachable
        // here in practice — kept explicit, not a wildcard, per the same
        // reasoning as `scan_capacity_for_summary`'s match above.
        RunStatus::Bootstrapping | RunStatus::Running | RunStatus::Parked => "running",
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
    // Task 12 M1: `Attention::Waiting` (parked on a provider rate limit)
    // gets its own group rather than falling through unfiltered by any of
    // the three buckets above/below — a run whose label matches none of
    // them would otherwise silently vanish from this listing entirely
    // (visible only via `--json`), the same "filter upstream drops a real
    // class of input" failure this project has hit before. A richer,
    // wake-time-aware surface is M5's job; this only keeps a parked run
    // visible.
    let waiting: Vec<&InboxEntry> = entries
        .iter()
        .filter(|e| e.attention == "waiting")
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
            print_capacity_line(e);
        }
    }

    if !waiting.is_empty() {
        println!("\n⏸ WAITING (parked on capacity) ({})", waiting.len());
        for e in &waiting {
            let node = e.active_node.as_deref().unwrap_or("-");
            println!("  {}  @{}", short_run(&e.run_id), node);
            print_capacity_line(e);
        }
    }

    println!("\n▶ WORKING ({})", working.len());
    for e in &working {
        let node = e.active_node.as_deref().unwrap_or("-");
        println!("  {}  @{}", short_run(&e.run_id), node);
        print_capacity_line(e);
    }

    if show_done {
        println!("\n✔ DONE ({})", done.len());
        for e in &done {
            println!(
                "  {}  {}",
                short_run(&e.run_id),
                e.done_reason.unwrap_or("done")
            );
            print_capacity_line(e);
        }
    } else {
        println!("\n✔ DONE: {} (use --all to list)", done.len());
    }
}

/// Print a rate-limit capacity line (R34–R36) under an inbox entry. Silent
/// for the common `NeverObserved` case; `Unclassified` still prints —
/// staying silent there would make "saw a failure, couldn't classify it"
/// read exactly like "nothing happened", the distinction `CapacityStatus`
/// exists to preserve.
fn print_capacity_line(entry: &InboxEntry) {
    match &entry.capacity {
        CapacityStatus::NeverObserved => {},
        CapacityStatus::Known(window) => {
            println!(
                "      ⚠ rate-limited: runtime={} resets_in={}",
                window.runtime(),
                format_resets_in(window, chrono::Utc::now())
            );
        },
        CapacityStatus::Unclassified => {
            println!(
                "      ⚠ a failure occurred that Surge could not classify as a rate limit \
                 (capacity unknown, not clean)"
            );
        },
    }
}

/// Render "time until reset" for a `Known` window, distinguishing three
/// facts that must not collapse into one: no `Retry-After` was ever
/// observed (`resets_at` itself is `None`) vs. one was observed and it is
/// still ahead vs. one was observed but has already elapsed with no
/// fresher signal since. `CapacityWindow::seconds_until_reset` alone
/// returns `None` for both the first and third case — exactly the
/// conflation `CapacityStatus` exists to refuse, so this reads
/// `resets_at()` directly to tell them apart before falling back to it.
fn format_resets_in(
    window: &surge_core::capacity::CapacityWindow,
    now: chrono::DateTime<chrono::Utc>,
) -> String {
    match (window.resets_at(), window.seconds_until_reset(now)) {
        (None, _) => "unknown".to_string(),
        (Some(_), Some(secs)) => format!("{secs}s"),
        (Some(_), None) => "elapsed (no fresher signal)".to_string(),
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
    use surge_core::{RunId, RunStatus, SessionId};

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
                budget: Default::default(),
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

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn inbox_surfaces_waiting_group_for_a_parked_run() {
        // Task 12 M1 (non-blocking review item): a parked run must land in
        // its own "waiting" group, not silently vanish from every printed
        // bucket (`needs_input`/`working`/`done` all filter by exact string
        // match — a fourth label that matches none of them would otherwise
        // only be visible via `--json`).
        use surge_core::capacity::WakeBasis;

        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("proj");
        let storage = Storage::open(dir.path()).await.unwrap();

        let parked = RunId::new();
        let w = storage.create_run(parked, &project, None).await.unwrap();
        let wake_at = chrono::Utc::now() + chrono::Duration::minutes(5);
        append(
            &w,
            vec![
                run_started(),
                pipeline_materialized(),
                EventPayload::RunParked {
                    wake_at,
                    runtime: Some("claude-acp".into()),
                    worktree: dir.path().to_path_buf(),
                    basis: WakeBasis::ObservedReset,
                    reason: "provider rate limit exhausted".into(),
                },
            ],
        )
        .await;
        w.flush().await.unwrap();

        let entries = collect_entries(&storage, Some(project.clone()), 100)
            .await
            .unwrap();
        let entry = entries
            .iter()
            .find(|e| e.run_id == parked.to_string())
            .expect("parked run present");
        assert_eq!(entry.attention, "waiting");
    }

    /// A real `SessionOpened` as `agent.rs` actually emits it: `agent` holds
    /// the flow-authored *profile* (`"implementer@1.0"`-shaped — a role, not
    /// a runtime), `agent_id` holds the real runtime identity. Two
    /// fixtures below deliberately give these *different* values so a test
    /// reading the wrong field is caught rather than passing by
    /// coincidence.
    fn session_opened(node: &str, runtime: &str) -> EventPayload {
        EventPayload::SessionOpened {
            node: NodeKey::try_from(node).unwrap(),
            session: SessionId::new(),
            agent: "implementer@1.0".into(),
            agent_id: Some(runtime.into()),
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn inbox_surfaces_capacity_signal_from_a_real_stage_failure() {
        // Boevoy put': the run's event log carries exactly what a real
        // rate-limited dispatch persists — `SessionOpened{agent, agent_id}`
        // then `StageFailed{reason}` with the provider's 429 text — and
        // `collect_entries` (the same path `surge inbox` calls) must surface
        // it without inventing a window length Surge never observed.
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("proj");
        let storage = Storage::open(dir.path()).await.unwrap();

        let run = RunId::new();
        let w = storage.create_run(run, &project, None).await.unwrap();
        append(
            &w,
            vec![
                run_started(),
                pipeline_materialized(),
                session_opened("plan", "claude-work"),
                EventPayload::StageFailed {
                    node: NodeKey::try_from("plan").unwrap(),
                    reason: "429 Too Many Requests; Retry-After: 45".into(),
                    retry_available: true,
                },
            ],
        )
        .await;
        w.flush().await.unwrap();

        let entries = collect_entries(&storage, Some(project.clone()), 100)
            .await
            .unwrap();
        let entry = entries
            .iter()
            .find(|e| e.run_id == run.to_string())
            .expect("run present");

        assert_eq!(entry.attention, "working");
        let window = entry
            .capacity
            .window()
            .expect("429 in StageFailed.reason must surface as a capacity signal");
        // The runtime is `agent_id` ("claude-work"), never the profile
        // string `agent` carries ("implementer@1.0") — the exact mix-up
        // that would read one real runtime under two profiles as two.
        assert_eq!(window.runtime(), "claude-work");
        assert!(window.is_exhausted());
        assert!(window.resets_at().is_some());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn inbox_capacity_is_unclassified_when_agent_id_is_absent() {
        // A run recorded before `agent_id` existed (or opened via the
        // no-profile-registry legacy path) decodes `agent_id` as `None` —
        // there is no runtime to attribute the failure to, so this must
        // read as "saw something, cannot classify it", not fabricate a
        // runtime from the profile string, and not silently disappear into
        // `NeverObserved` either.
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("proj");
        let storage = Storage::open(dir.path()).await.unwrap();

        let run = RunId::new();
        let w = storage.create_run(run, &project, None).await.unwrap();
        append(
            &w,
            vec![
                run_started(),
                pipeline_materialized(),
                EventPayload::SessionOpened {
                    node: NodeKey::try_from("plan").unwrap(),
                    session: SessionId::new(),
                    agent: "implementer@1.0".into(),
                    agent_id: None,
                },
                EventPayload::StageFailed {
                    node: NodeKey::try_from("plan").unwrap(),
                    reason: "429 Too Many Requests".into(),
                    retry_available: true,
                },
            ],
        )
        .await;
        w.flush().await.unwrap();

        let entries = collect_entries(&storage, Some(project.clone()), 100)
            .await
            .unwrap();
        let entry = entries
            .iter()
            .find(|e| e.run_id == run.to_string())
            .expect("run present");
        assert_eq!(entry.capacity, CapacityStatus::Unclassified);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn inbox_capacity_persists_when_a_different_node_completes() {
        // The reviewer's exact example: `plan` fails with a 429 on one
        // runtime, then a *different* node finishes — that must not erase
        // the still-open signal on `plan`. Reset is node-scoped, not
        // "any StageCompleted anywhere in the run".
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("proj");
        let storage = Storage::open(dir.path()).await.unwrap();

        let run = RunId::new();
        let w = storage.create_run(run, &project, None).await.unwrap();
        append(
            &w,
            vec![
                run_started(),
                pipeline_materialized(),
                session_opened("plan", "claude-work"),
                EventPayload::StageFailed {
                    node: NodeKey::try_from("plan").unwrap(),
                    reason: "429 Too Many Requests".into(),
                    retry_available: true,
                },
                session_opened("review", "claude-other"),
                EventPayload::StageCompleted {
                    node: NodeKey::try_from("review").unwrap(),
                    outcome: surge_core::OutcomeKey::try_from("done").unwrap(),
                },
            ],
        )
        .await;
        w.flush().await.unwrap();

        let entries = collect_entries(&storage, Some(project.clone()), 100)
            .await
            .unwrap();
        let entry = entries
            .iter()
            .find(|e| e.run_id == run.to_string())
            .expect("run present");
        let window = entry
            .capacity
            .window()
            .expect("an unrelated node completing must not clear plan's signal");
        assert_eq!(window.runtime(), "claude-work");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn inbox_capacity_known_on_one_node_survives_an_unrelated_node_failing_and_finishing() {
        // The scenario a single shared slot got wrong twice: `plan` fails
        // with a 429 (Known), then `review` separately fails with an
        // unrelated error. A single `status`/`failed_node` slot would
        // downgrade to `Unclassified` right there, *losing* `plan`'s signal
        // before `review` even completes. Per-node tracking must keep both
        // nodes' status independent: the merged result still surfaces
        // `plan`'s `Known`, and `review` finishing afterward removes only
        // `review`'s own (already-`Unclassified`) entry.
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("proj");
        let storage = Storage::open(dir.path()).await.unwrap();

        let run = RunId::new();
        let w = storage.create_run(run, &project, None).await.unwrap();
        append(
            &w,
            vec![
                run_started(),
                pipeline_materialized(),
                session_opened("plan", "claude-work"),
                EventPayload::StageFailed {
                    node: NodeKey::try_from("plan").unwrap(),
                    reason: "429 Too Many Requests".into(),
                    retry_available: true,
                },
                session_opened("review", "claude-other"),
                EventPayload::StageFailed {
                    node: NodeKey::try_from("review").unwrap(),
                    reason: "internal server error".into(),
                    retry_available: true,
                },
                EventPayload::StageCompleted {
                    node: NodeKey::try_from("review").unwrap(),
                    outcome: surge_core::OutcomeKey::try_from("done").unwrap(),
                },
            ],
        )
        .await;
        w.flush().await.unwrap();

        let entries = collect_entries(&storage, Some(project.clone()), 100)
            .await
            .unwrap();
        let entry = entries
            .iter()
            .find(|e| e.run_id == run.to_string())
            .expect("run present");
        let window = entry.capacity.window().expect(
            "plan's Known signal must survive review separately failing and then completing",
        );
        assert_eq!(window.runtime(), "claude-work");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn inbox_surfaces_capacity_signal_from_a_terminal_failed_run() {
        // The class the reviewer named: a run that failed *from* a 429 is
        // terminal (`RunStatus::Failed`) by construction — the signal lives
        // exactly in the class `classify`'s terminal short-circuit used to
        // skip entirely, never in the non-terminal class the scan
        // originally covered. This proves the fix, not just the model.
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("proj");
        let storage = Storage::open(dir.path()).await.unwrap();

        let run = RunId::new();
        let w = storage.create_run(run, &project, None).await.unwrap();
        append(
            &w,
            vec![
                run_started(),
                pipeline_materialized(),
                session_opened("plan", "claude-work"),
                EventPayload::StageFailed {
                    node: NodeKey::try_from("plan").unwrap(),
                    reason: "429 Too Many Requests; Retry-After: 45".into(),
                    retry_available: false,
                },
                EventPayload::RunFailed {
                    error: "rate limited".into(),
                },
            ],
        )
        .await;
        w.flush().await.unwrap();
        storage
            .set_run_status(&run, RunStatus::Failed, Some(1))
            .await
            .unwrap();

        let entries = collect_entries(&storage, Some(project.clone()), 100)
            .await
            .unwrap();
        let entry = entries
            .iter()
            .find(|e| e.run_id == run.to_string())
            .expect("run present");

        assert_eq!(entry.attention, "done");
        let window = entry
            .capacity
            .window()
            .expect("a terminal Failed run's own StageFailed must still surface");
        assert_eq!(window.runtime(), "claude-work");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn inbox_capacity_is_never_observed_when_no_failure_occurred() {
        // The primary case (R35.1): an ordinary run carries no rate-limit
        // signal, and this must read as absent, never a guess.
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("proj");
        let storage = Storage::open(dir.path()).await.unwrap();

        let run = RunId::new();
        let w = storage.create_run(run, &project, None).await.unwrap();
        append(&w, vec![run_started(), pipeline_materialized()]).await;
        w.flush().await.unwrap();

        let entries = collect_entries(&storage, Some(project.clone()), 100)
            .await
            .unwrap();
        let entry = entries
            .iter()
            .find(|e| e.run_id == run.to_string())
            .expect("run present");
        assert_eq!(entry.capacity, CapacityStatus::NeverObserved);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn inbox_capacity_is_unclassified_for_an_unrecognized_failure() {
        // A real failure happened, but its text matches none of
        // `RATE_LIMIT_PATTERNS` — this must not read the same as
        // `NeverObserved` (the exact defect the second review round named).
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("proj");
        let storage = Storage::open(dir.path()).await.unwrap();

        let run = RunId::new();
        let w = storage.create_run(run, &project, None).await.unwrap();
        append(
            &w,
            vec![
                run_started(),
                pipeline_materialized(),
                session_opened("plan", "claude-work"),
                EventPayload::StageFailed {
                    node: NodeKey::try_from("plan").unwrap(),
                    reason: "syntax error: unexpected token".into(),
                    retry_available: true,
                },
            ],
        )
        .await;
        w.flush().await.unwrap();

        let entries = collect_entries(&storage, Some(project.clone()), 100)
            .await
            .unwrap();
        let entry = entries
            .iter()
            .find(|e| e.run_id == run.to_string())
            .expect("run present");
        assert_eq!(entry.capacity, CapacityStatus::Unclassified);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn inbox_capacity_clears_after_a_later_stage_completes() {
        // A rate-limited retry that then succeeds is no longer evidence of
        // an ongoing problem.
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("proj");
        let storage = Storage::open(dir.path()).await.unwrap();

        let run = RunId::new();
        let w = storage.create_run(run, &project, None).await.unwrap();
        append(
            &w,
            vec![
                run_started(),
                pipeline_materialized(),
                session_opened("plan", "claude-work"),
                EventPayload::StageFailed {
                    node: NodeKey::try_from("plan").unwrap(),
                    reason: "429 Too Many Requests".into(),
                    retry_available: true,
                },
                EventPayload::StageCompleted {
                    node: NodeKey::try_from("plan").unwrap(),
                    outcome: surge_core::OutcomeKey::try_from("done").unwrap(),
                },
            ],
        )
        .await;
        w.flush().await.unwrap();

        let entries = collect_entries(&storage, Some(project.clone()), 100)
            .await
            .unwrap();
        let entry = entries
            .iter()
            .find(|e| e.run_id == run.to_string())
            .expect("run present");
        assert_eq!(entry.capacity, CapacityStatus::NeverObserved);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn inbox_surfaces_capacity_signal_from_a_terminal_aborted_run() {
        // An operator can abort after watching repeated rate-limit
        // retries — the signal lives in `Aborted` just as plausibly as in
        // `Failed`.
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("proj");
        let storage = Storage::open(dir.path()).await.unwrap();

        let run = RunId::new();
        let w = storage.create_run(run, &project, None).await.unwrap();
        append(
            &w,
            vec![
                run_started(),
                pipeline_materialized(),
                session_opened("plan", "claude-work"),
                EventPayload::StageFailed {
                    node: NodeKey::try_from("plan").unwrap(),
                    reason: "429 Too Many Requests".into(),
                    retry_available: true,
                },
                EventPayload::RunAborted {
                    reason: "operator gave up".into(),
                },
            ],
        )
        .await;
        w.flush().await.unwrap();
        storage
            .set_run_status(&run, RunStatus::Aborted, Some(1))
            .await
            .unwrap();

        let entries = collect_entries(&storage, Some(project.clone()), 100)
            .await
            .unwrap();
        let entry = entries
            .iter()
            .find(|e| e.run_id == run.to_string())
            .expect("run present");
        assert_eq!(entry.attention, "done");
        let window = entry
            .capacity
            .window()
            .expect("an Aborted run's own StageFailed must still surface");
        assert_eq!(window.runtime(), "claude-work");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn inbox_surfaces_capacity_signal_from_a_terminal_crashed_run() {
        // `Crashed` is assigned by `Storage::list_runs` rewriting a dead
        // daemon's `Running`/`Bootstrapping` run — a label about the
        // daemon's liveness, not the pipeline outcome. A run that hit a 429
        // and then had its daemon die carries the same `StageFailed` in its
        // log and must not be skipped because the status differs from
        // `Failed`.
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("proj");
        let storage = Storage::open(dir.path()).await.unwrap();

        let run = RunId::new();
        let w = storage.create_run(run, &project, None).await.unwrap();
        append(
            &w,
            vec![
                run_started(),
                pipeline_materialized(),
                session_opened("plan", "claude-work"),
                EventPayload::StageFailed {
                    node: NodeKey::try_from("plan").unwrap(),
                    reason: "429 Too Many Requests".into(),
                    retry_available: true,
                },
            ],
        )
        .await;
        w.flush().await.unwrap();
        // No corresponding event: `Crashed` has no event of its own — it is
        // a registry-level status `Storage::list_runs` assigns directly.
        storage
            .set_run_status(&run, RunStatus::Crashed, None)
            .await
            .unwrap();

        let entries = collect_entries(&storage, Some(project.clone()), 100)
            .await
            .unwrap();
        let entry = entries
            .iter()
            .find(|e| e.run_id == run.to_string())
            .expect("run present");
        assert_eq!(entry.attention, "done");
        let window = entry
            .capacity
            .window()
            .expect("a Crashed run's own StageFailed must still surface");
        assert_eq!(window.runtime(), "claude-work");
    }

    #[test]
    fn format_resets_in_distinguishes_unknown_from_elapsed() {
        use surge_core::capacity::CapacityWindow;

        let now = chrono::Utc::now();

        // Never observed a Retry-After at all.
        let never_had_one = CapacityWindow::observed_429("claude", None, now);
        assert_eq!(format_resets_in(&never_had_one, now), "unknown");

        // Had one, and it has already passed — a different fact from the
        // above, and must render differently, not both as "unknown".
        let already_elapsed = CapacityWindow::observed_429(
            "claude",
            Some(std::time::Duration::from_secs(30)),
            now - chrono::Duration::seconds(60),
        );
        assert_eq!(
            format_resets_in(&already_elapsed, now),
            "elapsed (no fresher signal)"
        );

        // Had one, still ahead.
        let still_ahead =
            CapacityWindow::observed_429("claude", Some(std::time::Duration::from_secs(60)), now);
        assert_eq!(format_resets_in(&still_ahead, now), "60s");
    }
}
