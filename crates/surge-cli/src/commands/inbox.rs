//! `surge inbox` — the fleet inbox: every run grouped by what it needs from
//! the operator right now (Needs input / Working / Done), blocked-first
//! (Phase 2 B1).
//!
//! There is no persisted "blocked on human" flag today, so attention is derived
//! authoritatively by folding each non-terminal run's event log into a
//! `RunState` and classifying it. Attention for a terminal run is read cheaply
//! from the registry status alone (no fold).
//!
//! **The capacity column is a registry fact, not a run fact (Task 12 M5).**
//! Before this milestone, `scan_capacity_signal` re-derived "is a rate limit
//! in play" by folding *this run's own* event log for `StageFailed`/
//! `SessionOpened` — a second full `read_events(0..MAX)` alongside the
//! attention fold for every non-terminal run, and a fresh reader + full read
//! for every terminal `Failed`/`Aborted`/`Crashed` run besides. That was two
//! homes for one fact and they disagreed on the first opportunity: run A's
//! own journal knows nothing of the 429 run B took on the *same* runtime, so
//! A's column stayed silent while the scheduler had already parked on that
//! exact exhaustion. Capacity is a fact about a runtime/account, not about
//! any one run's history, so it now comes from exactly one place — the
//! registry-level `runtime_capacity` table (`surge_persistence::runs::
//! capacity`), the same table the scheduler itself parks from — via a
//! canonical-keyed point lookup (`CanonicalRuntimeId::resolve` +
//! `Storage::capacity_status`), never a raw string and never a per-run scan.
//! See [`classify`]'s doc for exactly which `RunStatus` classes this
//! populates the column for, and why the answer is narrower than before.

use std::path::PathBuf;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use clap::Args;
use serde::Serialize;
use surge_core::capacity::{CapacityStatus, WakeBasis};
use surge_core::{Attention, RunState, TerminalReason};
use surge_orchestrator::engine::capacity::CanonicalRuntimeId;
use surge_persistence::runs::Storage;
use surge_persistence::runs::registry::{RunFilter, RunSummary};

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
    /// Rate-limit capacity signal (R34–R36) for the runtime this run is
    /// currently parked on — a registry point-lookup (`runtime_capacity`),
    /// **not** anything folded from this run's own event log; see the
    /// module doc and [`classify`] for which `RunStatus`/`Attention` classes
    /// this is ever populated for. `NeverObserved` is the common case
    /// (R35.1); `Unclassified` (the registry saw *something* for this
    /// runtime it could not decode) is kept distinct from it rather than
    /// folded into the same silence — see `CapacityStatus`'s doc.
    #[serde(skip_serializing_if = "CapacityStatus::is_never_observed")]
    capacity: CapacityStatus,
    /// When a parked run (`attention == "waiting"`) is expected to resume on
    /// its own. `None` for every other attention — a run that is not parked
    /// has no wake time to show, not an unknown one. (Task 12 M5.)
    #[serde(skip_serializing_if = "Option::is_none")]
    wake_at: Option<DateTime<Utc>>,
    /// Why `wake_at` is what it is — an actually-observed provider reset vs.
    /// a configured blind-backoff guess. Carried alongside `wake_at` rather
    /// than folded into a display string, so a machine consumer of
    /// `--format json` learns the same typed fact an operator reading the
    /// text output sees, not just that the run is parked. `None` exactly
    /// when `wake_at` is `None`.
    #[serde(skip_serializing_if = "Option::is_none")]
    wake_basis: Option<WakeBasis>,
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
    // inbox. This is not free — every non-terminal run still costs a full
    // `read_events(0..MAX)` for its attention fold (unrelated to the capacity
    // column; see the module doc) — but that cost is accepted debt for a
    // handful of active runs, not a reason to risk dropping one from NEEDS
    // INPUT by pre-truncating before classification. Terminal runs cost no
    // event read at all, for attention or capacity alike.
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
/// (state derivation is cheap from the registry alone) — and, since Task 12
/// M5, so does the capacity column: **every** `RunStatus` reads
/// `CapacityStatus::NeverObserved` for it except the one class where a
/// specific runtime is both cheaply known and actually relevant. Named per
/// variant, not left to "the rest by construction carry no signal" (that
/// reasoning was wrong once already for this exact column — see
/// `surge-runstatus-crashed-filter-trap` in project memory):
///
/// - `Parked` (`Attention::Waiting`): **the only class that populates the
///   column.** A parked run's own `RunParked.runtime` (already-canonical,
///   folded into `Attention::Waiting.runtime`) says *which* runtime it is
///   waiting on; [`runtime_capacity_status`] then asks the registry what it
///   currently knows about that runtime. This is the class the signal is
///   actually load-bearing for — it is *why* the run is in this group.
/// - `Bootstrapping` / `Running`: reached only through the non-terminal
///   branch below, folding to `Attention::Working` or `NeedsInput` in
///   practice (parking only ever happens right before a pipeline node
///   dispatch — see `engine::run_task`). Reads `NeverObserved`: the
///   dispatch gate (`capacity_decision_for`) already runs before every
///   agent stage, so a run that is *not* currently parked is, by that
///   gate's own guarantee, not blocked by capacity right now — showing a
///   runtime's exhaustion next to it would describe a different run's
///   problem, not this one's.
/// - `Failed` / `Aborted` / `Crashed`: **no longer scanned — a deliberate
///   narrowing, not an oversight.** The old per-run scan treated these as
///   its primary class (a run that failed *from* a 429 is `Failed` by
///   construction); that reasoning conflated "this run's own dead history"
///   with "the runtime's live status," which is exactly the two-homes bug
///   this milestone closes (see the module doc). A terminal run gets no
///   reader opened and no event read at all for this column now — there is
///   no cheap, correct way to attribute a *specific* runtime to a run that
///   will never dispatch again, and guessing one from a registry row that
///   happens to exist would be the same unattributed inference the old scan
///   is being replaced for. `Crashed` (a daemon-liveness label
///   `Storage::list_runs` assigns, not a pipeline outcome) is named
///   explicitly, not folded into "the rest," precisely because a prior
///   round of this project got exactly that shortcut wrong.
/// - `Completed`: `NeverObserved`, as before — a run that reached its own
///   terminal success node needs no capacity accounting at all.
async fn classify(storage: &std::sync::Arc<Storage>, summary: &RunSummary) -> Result<InboxEntry> {
    let base = |attention: &'static str, done_reason, active_node, prompt, capacity| InboxEntry {
        run_id: summary.id.to_string(),
        project_path: summary.project_path.clone(),
        attention,
        done_reason,
        active_node,
        prompt,
        capacity,
        // Set only by the `Attention::Waiting` arm below, via struct-update
        // syntax — every other attention has no wake time to show.
        wake_at: None,
        wake_basis: None,
        started_at_ms: summary.started_at_ms,
    };

    if summary.status.is_terminal() {
        // No reader opened, no event read, for any terminal status —
        // `Completed` included, exactly as before, and `Failed`/`Aborted`/
        // `Crashed` now alike (see this fn's own doc for why that is a
        // deliberate narrowing of the old scan's class, not a regression).
        return Ok(base(
            "done",
            Some(terminal_label(summary.status)),
            None,
            None,
            CapacityStatus::NeverObserved,
        ));
    }

    // Non-terminal: fold the event log for the authoritative attention state.
    let reader = storage
        .open_run_reader(summary.id)
        .await
        .with_context(|| format!("open run {}", summary.id))?;
    let state = fold_run_state(&reader, summary.id).await?;
    let active_node = active_node(&state);
    let attention = state.attention();
    // Capacity is a registry point-lookup keyed on *this run's own* parked
    // runtime, not a fold over its journal — see the module doc. Every
    // attention other than `Waiting` reads `NeverObserved`; see `classify`'s
    // own doc for why that is the correct class, not merely the cheap one.
    let capacity = match &attention {
        Attention::Waiting {
            runtime: Some(raw_runtime),
            ..
        } => runtime_capacity_status(storage, raw_runtime).await,
        Attention::Waiting { runtime: None, .. }
        | Attention::NeedsInput
        | Attention::Working
        | Attention::Done(_) => CapacityStatus::NeverObserved,
    };
    Ok(match attention {
        Attention::NeedsInput => base(
            "needs_input",
            None,
            active_node,
            state.pending_prompt().map(ToOwned::to_owned),
            capacity,
        ),
        Attention::Working => base("working", None, active_node, None, capacity),
        // Task 12 M5: the wake time and its basis (observed provider reset
        // vs. a policy-backoff guess) ride along on the entry itself, typed,
        // rather than only being visible as the "waiting" label — see
        // `InboxEntry::wake_at`/`wake_basis`.
        Attention::Waiting { until, basis, .. } => InboxEntry {
            wake_at: Some(until),
            wake_basis: Some(basis),
            ..base("waiting", None, active_node, None, capacity)
        },
        Attention::Done(reason) => base("done", Some(reason_label(reason)), None, None, capacity),
    })
}

/// Point-read the registry's current capacity status for the runtime a
/// parked run recorded in its own `RunParked.runtime` (Task 12 M5) — the
/// registry-fact half of the module doc's split. `raw_runtime` has always
/// been canonical since `RunParked`'s introduction (every write site builds
/// it via `CanonicalRuntimeId::resolve` — see `engine::run_task`), unlike
/// the legacy `SessionOpened.agent_id` the old per-run scan keyed off,
/// which predates that write-site fix and can still carry a raw, un-
/// normalized value on a run started before it landed (ADR-0016, M1). This
/// function does not inherit that risk — it never reads `agent_id` at
/// all — but re-resolves through [`CanonicalRuntimeId::resolve`] anyway
/// rather than trusting the string, the same discipline the write side
/// (`CapacityLedger::observe`) already applies: the read half of a typed
/// contract should not depend on every writer, past or future, having
/// gotten it right.
///
/// A read failure (registry unreachable) degrades to `NeverObserved` —
/// best-effort, same as the scan this replaces: one run's optional capacity
/// line must never block the rest of the inbox listing.
async fn runtime_capacity_status(
    storage: &std::sync::Arc<Storage>,
    raw_runtime: &str,
) -> CapacityStatus {
    let canonical = CanonicalRuntimeId::resolve(&surge_acp::Registry::builtin(), raw_runtime);
    storage
        .capacity_status(canonical.as_str())
        .await
        .unwrap_or_else(|err| {
            tracing::debug!(
                runtime = %canonical,
                error = %err,
                "capacity status read failed; showing none"
            );
            CapacityStatus::NeverObserved
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
        // Only ever called with `is_terminal()` statuses (see the call
        // site above); `Bootstrapping`/`Running`/`Parked` are unreachable
        // here in practice — kept explicit, not a wildcard, per the same
        // reasoning `classify`'s own doc gives for enumerating every
        // `RunStatus` by name rather than falling through to "the rest."
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
    print_inbox_to(&mut std::io::stdout().lock(), entries, show_done);
}

/// Render the grouped inbox listing to `out`. Split from [`print_inbox`]
/// so a test can capture and assert on the exact printed bytes — including
/// the WAITING group's own heading and wake-time line — without spawning a
/// subprocess. A write failure (e.g. `BrokenPipe` from `| head`) is
/// swallowed per line rather than aborting the rest of the listing: this is
/// best-effort terminal output, not a fallible operation with a caller that
/// could do anything useful with the error.
fn print_inbox_to(out: &mut impl std::io::Write, entries: &[InboxEntry], show_done: bool) {
    let needs: Vec<&InboxEntry> = entries
        .iter()
        .filter(|e| e.attention == "needs_input")
        .collect();
    let working: Vec<&InboxEntry> = entries
        .iter()
        .filter(|e| e.attention == "working")
        .collect();
    // `Attention::Waiting` (parked on a provider rate limit) gets its own
    // group rather than falling through unfiltered by any of the three
    // buckets above/below — a run whose label matches none of them would
    // otherwise silently vanish from this listing entirely (visible only
    // via `--json`), the same "filter upstream drops a real class of input"
    // failure this project has hit before. This block itself is what
    // guards that: deleting it (rather than merely emptying `waiting`)
    // removes both the group and the parked run's only non-JSON visibility
    // — pinned by `print_inbox_waiting_group_survives_deleting_this_block_
    // fails` below.
    let waiting: Vec<&InboxEntry> = entries
        .iter()
        .filter(|e| e.attention == "waiting")
        .collect();
    let done: Vec<&InboxEntry> = entries.iter().filter(|e| e.attention == "done").collect();

    // Blocked-first: the "needs me right now" group leads.
    let _ = writeln!(out, "⚑ NEEDS INPUT ({})", needs.len());
    if needs.is_empty() {
        let _ = writeln!(out, "  (nothing waiting on you)");
    } else {
        for e in &needs {
            let node = e.active_node.as_deref().unwrap_or("-");
            let _ = writeln!(out, "  {}  @{}", short_run(&e.run_id), node);
            if let Some(prompt) = &e.prompt {
                let _ = writeln!(out, "      ↳ {}", first_line(prompt));
            }
            print_capacity_line(out, e);
        }
    }

    if !waiting.is_empty() {
        let _ = writeln!(out, "\n⏸ WAITING (parked on capacity) ({})", waiting.len());
        for e in &waiting {
            let node = e.active_node.as_deref().unwrap_or("-");
            let _ = writeln!(out, "  {}  @{}", short_run(&e.run_id), node);
            let _ = writeln!(out, "      ↻ {}", format_wake_line(e));
            print_capacity_line(out, e);
        }
    }

    let _ = writeln!(out, "\n▶ WORKING ({})", working.len());
    for e in &working {
        let node = e.active_node.as_deref().unwrap_or("-");
        let _ = writeln!(out, "  {}  @{}", short_run(&e.run_id), node);
        print_capacity_line(out, e);
    }

    if show_done {
        let _ = writeln!(out, "\n✔ DONE ({})", done.len());
        for e in &done {
            let _ = writeln!(
                out,
                "  {}  {}",
                short_run(&e.run_id),
                e.done_reason.unwrap_or("done")
            );
            print_capacity_line(out, e);
        }
    } else {
        let _ = writeln!(out, "\n✔ DONE: {} (use --all to list)", done.len());
    }
}

/// Render a parked entry's wake time and why it is what it is (Task 12 M5):
/// an operator needs to see *when* a parked run resumes on its own and
/// *why* that time was chosen — an actual provider-observed reset vs. a
/// configured blind-backoff guess — not just that the run is parked.
/// `entry.wake_at`/`wake_basis` are set together or not at all (see
/// `classify`'s `Attention::Waiting` arm), so the `None` arms below are
/// defensive, not an expected split state.
fn format_wake_line(entry: &InboxEntry) -> String {
    let basis_label = match entry.wake_basis {
        Some(WakeBasis::ObservedReset) => "observed provider reset",
        Some(WakeBasis::PolicyBackoff) => "policy backoff (no reset observed)",
        None => "basis unknown",
    };
    match entry.wake_at {
        Some(wake_at) => format!("wakes at {} ({basis_label})", wake_at.to_rfc3339()),
        None => "wake time unknown".to_string(),
    }
}

/// Print a rate-limit capacity line (R34–R36) under an inbox entry. Silent
/// for the common `NeverObserved` case; `Unclassified` still prints —
/// staying silent there would make "saw a failure, couldn't classify it"
/// read exactly like "nothing happened", the distinction `CapacityStatus`
/// exists to preserve.
fn print_capacity_line(out: &mut impl std::io::Write, entry: &InboxEntry) {
    match &entry.capacity {
        CapacityStatus::NeverObserved => {},
        CapacityStatus::Known(window) => {
            let _ = writeln!(
                out,
                "      ⚠ rate-limited: runtime={} resets_in={}",
                window.runtime(),
                format_resets_in(window, chrono::Utc::now())
            );
        },
        CapacityStatus::Unclassified => {
            let _ = writeln!(
                out,
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
        // Task 12 M5: the wake time and its basis must ride along on the
        // entry itself — an operator (or a `--json` consumer) must be able
        // to answer "when does this come back, and why" without re-deriving
        // it from the run's event log.
        assert_eq!(entry.wake_at, Some(wake_at));
        assert_eq!(entry.wake_basis, Some(WakeBasis::ObservedReset));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn inbox_json_output_carries_wake_at_and_basis_for_a_parked_run() {
        // Task 12 M5 acceptance: a machine consumer of `--format json` must
        // not have to fall back to the text output (or re-scan the run's
        // event log) to learn a run is parked and when/why it wakes — the
        // same typed fact the printed WAITING group shows must be present
        // in the serialized entry.
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
                    basis: WakeBasis::PolicyBackoff,
                    reason: "blind backoff, no observed reset time".into(),
                },
            ],
        )
        .await;
        w.flush().await.unwrap();

        let entries = collect_entries(&storage, Some(project.clone()), 100)
            .await
            .unwrap();
        let json = serde_json::to_value(&entries).unwrap();
        let entry = json
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["run_id"] == parked.to_string())
            .expect("parked run present in JSON output");

        assert_eq!(entry["attention"], "waiting");
        assert_eq!(entry["wake_at"], serde_json::json!(wake_at));
        assert_eq!(entry["wake_basis"], serde_json::json!("PolicyBackoff"));
    }

    #[test]
    fn print_inbox_waiting_group_shows_wake_time_and_basis() {
        // Mutation coverage (Task 12 M5 acceptance): before this test, the
        // entire `if !waiting.is_empty() { .. }` block in `print_inbox_to`
        // could be deleted outright and every existing `surge-cli` test
        // still passed — the prior tests only asserted on `entry.attention`
        // (a classification fact), never on what `print_inbox` actually
        // prints. This test asserts on the printed bytes directly, so
        // deleting that block removes the WAITING heading and the wake-time
        // line, and this test goes red.
        let wake_at = chrono::Utc::now() + chrono::Duration::minutes(5);
        let entry = InboxEntry {
            run_id: "01ABCDEFPARKEDRUNID12345".into(),
            project_path: PathBuf::from("/proj"),
            attention: "waiting",
            done_reason: None,
            active_node: Some("plan".into()),
            prompt: None,
            capacity: CapacityStatus::NeverObserved,
            wake_at: Some(wake_at),
            wake_basis: Some(WakeBasis::ObservedReset),
            started_at_ms: 0,
        };

        let mut out = Vec::new();
        print_inbox_to(&mut out, std::slice::from_ref(&entry), false);
        let text = String::from_utf8(out).unwrap();

        assert!(
            text.contains("WAITING"),
            "the WAITING group heading must be printed for a parked entry, got: {text}"
        );
        assert!(
            text.contains(&wake_at.to_rfc3339()),
            "the wake time must be printed, got: {text}"
        );
        assert!(
            text.contains("observed provider reset"),
            "the wake basis must be printed, got: {text}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn inbox_attention_leaves_waiting_once_the_run_wakes() {
        // Task 12 M4: the other half of the sibling test above —
        // `RunWokeFromPark` must clear `RunState::Pipeline.parked` in the
        // fold, or a resumed run keeps showing "waiting" in `surge inbox`
        // while it is actually executing again.
        use surge_core::capacity::WakeBasis;

        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("proj");
        let storage = Storage::open(dir.path()).await.unwrap();

        let run = RunId::new();
        let w = storage.create_run(run, &project, None).await.unwrap();
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
                EventPayload::RunWokeFromPark {},
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
        assert_eq!(
            entry.attention, "working",
            "RunWokeFromPark must clear the parked state — a resumed run must not still show \
             as \"waiting\" in the inbox"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn inbox_waiting_run_capacity_reads_from_the_registry_not_the_journal() {
        // Task 12 M5: the positive case for the whole redesign. This run's
        // *own* journal never carries a `StageFailed`/window of any kind —
        // only `RunParked` (a run fact: which runtime, and until when).
        // `entry.capacity` can therefore only become `Known` by reading the
        // registry's `runtime_capacity` table, proving the column's source
        // moved rather than merely asserting it did.
        use surge_core::capacity::{CapacityWindow, WakeBasis};

        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("proj");
        let storage = Storage::open(dir.path()).await.unwrap();

        let observed_at = chrono::Utc::now();
        storage
            .observe_capacity(&CapacityWindow::observed_429(
                "claude-acp",
                Some(std::time::Duration::from_secs(45)),
                observed_at,
            ))
            .await
            .unwrap();

        let run = RunId::new();
        let w = storage.create_run(run, &project, None).await.unwrap();
        let wake_at = observed_at + chrono::Duration::seconds(45);
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
            .find(|e| e.run_id == run.to_string())
            .expect("run present");
        assert_eq!(entry.attention, "waiting");
        let window = entry.capacity.window().expect(
            "the registry's observation must surface even though this run's own \
                     journal carries no StageFailed at all",
        );
        assert_eq!(window.runtime(), "claude-acp");
        assert!(window.is_exhausted());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn inbox_two_runs_parked_on_the_same_runtime_converge_through_the_registry() {
        // The exact divergence this milestone closes (module doc): before
        // it, run A's own journal knew nothing of a 429 run B observed on
        // the *same* runtime, so their capacity columns disagreed. Now both
        // read the one registry row, so they agree — proven here with run A
        // parked under a raw, unnormalized alias ("claude") and run B under
        // the canonical spelling ("claude-acp"), so this also proves
        // `CanonicalRuntimeId::resolve` normalizes at read time rather than
        // trusting the journal's own string.
        use surge_core::capacity::{CapacityWindow, WakeBasis};

        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("proj");
        let storage = Storage::open(dir.path()).await.unwrap();

        let observed_at = chrono::Utc::now();
        storage
            .observe_capacity(&CapacityWindow::observed_429(
                "claude-acp",
                Some(std::time::Duration::from_secs(60)),
                observed_at,
            ))
            .await
            .unwrap();

        let wake_at = observed_at + chrono::Duration::seconds(60);
        let park_event = |runtime: &str| EventPayload::RunParked {
            wake_at,
            runtime: Some(runtime.into()),
            worktree: dir.path().to_path_buf(),
            basis: WakeBasis::ObservedReset,
            reason: "provider rate limit exhausted".into(),
        };

        let run_a = RunId::new();
        let wa = storage.create_run(run_a, &project, None).await.unwrap();
        append(
            &wa,
            vec![run_started(), pipeline_materialized(), park_event("claude")],
        )
        .await;
        wa.flush().await.unwrap();

        let run_b = RunId::new();
        let wb = storage.create_run(run_b, &project, None).await.unwrap();
        append(
            &wb,
            vec![
                run_started(),
                pipeline_materialized(),
                park_event("claude-acp"),
            ],
        )
        .await;
        wb.flush().await.unwrap();

        let entries = collect_entries(&storage, Some(project.clone()), 100)
            .await
            .unwrap();
        let find = |id: RunId| {
            entries
                .iter()
                .find(|e| e.run_id == id.to_string())
                .unwrap_or_else(|| panic!("missing {id}"))
        };
        let window_a = find(run_a).capacity.window().expect(
            "run A must see the registry's observation despite its own RunParked \
                     recording the unnormalized alias \"claude\"",
        );
        let window_b = find(run_b)
            .capacity
            .window()
            .expect("run B must see the same observation");
        assert_eq!(
            window_a, window_b,
            "two runs parked on the same runtime must read the identical registry fact, not \
             two independently-derived ones"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn inbox_waiting_run_with_no_recorded_runtime_shows_never_observed() {
        // Task 12 plan point 10: absence of a runtime is absence of a fact,
        // not a fabricated key — a run parked via the legacy
        // no-profile-registry path (`RunParked.runtime: None`) has nothing
        // to look up, even though the registry has a real row for some
        // *other* runtime.
        use surge_core::capacity::{CapacityWindow, WakeBasis};

        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("proj");
        let storage = Storage::open(dir.path()).await.unwrap();

        storage
            .observe_capacity(&CapacityWindow::observed_429(
                "claude-acp",
                None,
                chrono::Utc::now(),
            ))
            .await
            .unwrap();

        let run = RunId::new();
        let w = storage.create_run(run, &project, None).await.unwrap();
        let wake_at = chrono::Utc::now() + chrono::Duration::minutes(5);
        append(
            &w,
            vec![
                run_started(),
                pipeline_materialized(),
                EventPayload::RunParked {
                    wake_at,
                    runtime: None,
                    worktree: dir.path().to_path_buf(),
                    basis: WakeBasis::PolicyBackoff,
                    reason: "blind backoff, no observed reset time".into(),
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
        assert_eq!(entry.attention, "waiting");
        assert_eq!(entry.capacity, CapacityStatus::NeverObserved);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn inbox_terminal_failed_run_never_consults_the_registry() {
        // Task 12 M5, deliberate narrowing (see `classify`'s own doc): a
        // terminal run gets no reader opened and no registry lookup for
        // this column at all, even when a real, matching registry row
        // exists — a dead run has no cheap, correct way to be attributed to
        // a specific runtime, so it must not *appear* to inherit one. This
        // is the mutation-sensitive half of the redesign: without the
        // terminal branch's unconditional `NeverObserved`, this test would
        // go red the moment a future change tried reading the registry for
        // a terminal run too.
        use surge_core::capacity::CapacityWindow;

        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("proj");
        let storage = Storage::open(dir.path()).await.unwrap();

        storage
            .observe_capacity(&CapacityWindow::observed_429(
                "claude-acp",
                Some(std::time::Duration::from_secs(30)),
                chrono::Utc::now(),
            ))
            .await
            .unwrap();

        let run = RunId::new();
        let w = storage.create_run(run, &project, None).await.unwrap();
        append(
            &w,
            vec![
                run_started(),
                pipeline_materialized(),
                EventPayload::StageFailed {
                    node: NodeKey::try_from("plan").unwrap(),
                    reason: "429 Too Many Requests; Retry-After: 30".into(),
                    retry_available: false,
                },
                EventPayload::RunFailed {
                    error: "rate limited".into(),
                },
            ],
        )
        .await;
        w.flush().await.unwrap();
        // Every terminal status shares the same unconditional branch in
        // `classify` (no per-variant match inside it) — `Failed` stands in
        // for `Aborted`/`Crashed`/`Completed` alike; see that fn's doc.
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
        assert_eq!(
            entry.capacity,
            CapacityStatus::NeverObserved,
            "a terminal run must never surface a registry row, even one that matches its own \
             last-known runtime"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn inbox_working_run_never_consults_the_registry() {
        // The non-terminal counterpart to the test above: a run that is
        // `Working` (not currently parked) is, by the dispatch gate's own
        // guarantee (`capacity_decision_for` runs before every agent
        // stage), not blocked by capacity right now — so it reads
        // `NeverObserved` even with a matching registry row in play. Only
        // `Attention::Waiting` ever triggers the lookup; see `classify`'s
        // doc for the full `RunStatus` accounting this covers the
        // `Bootstrapping`/`Running` classes for.
        use surge_core::capacity::CapacityWindow;

        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("proj");
        let storage = Storage::open(dir.path()).await.unwrap();

        storage
            .observe_capacity(&CapacityWindow::observed_429(
                "claude-acp",
                Some(std::time::Duration::from_secs(30)),
                chrono::Utc::now(),
            ))
            .await
            .unwrap();

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
        assert_eq!(entry.attention, "working");
        assert_eq!(entry.capacity, CapacityStatus::NeverObserved);
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
