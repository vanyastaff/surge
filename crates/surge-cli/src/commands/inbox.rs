//! `surge inbox` — the fleet inbox: every run grouped by what it needs from
//! the operator right now (Needs input / Working / Waiting / Done),
//! blocked-first (Phase 2 B1).
//!
//! Classification lives in [`surge_orchestrator::operator::inbox`]; this module
//! is the clap surface and the terminal rendering.

use anyhow::{Context, Result};
use clap::Args;
use surge_core::RunId;
use surge_core::capacity::{CapacityStatus, WakeBasis};
use surge_orchestrator::operator::{AttentionGroup, DoneReason, InboxEntry, collect_entries};
use surge_persistence::runs::Storage;

use crate::commands::common::{operator_failure, surge_home_dir};

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
    let entries = collect_entries(&storage, None, args.limit)
        .await
        .map_err(operator_failure)?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&entries)?);
    } else {
        print_inbox(&entries, args.all);
    }
    Ok(())
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
        .filter(|e| e.attention == AttentionGroup::NeedsInput)
        .collect();
    let working: Vec<&InboxEntry> = entries
        .iter()
        .filter(|e| e.attention == AttentionGroup::Working)
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
        .filter(|e| e.attention == AttentionGroup::Waiting)
        .collect();
    let done: Vec<&InboxEntry> = entries
        .iter()
        .filter(|e| e.attention == AttentionGroup::Done)
        .collect();

    // Blocked-first: the "needs me right now" group leads.
    let _ = writeln!(out, "⚑ NEEDS INPUT ({})", needs.len());
    if needs.is_empty() {
        let _ = writeln!(out, "  (nothing waiting on you)");
    } else {
        for e in &needs {
            let node = e.active_node.as_deref().unwrap_or("-");
            let _ = writeln!(out, "  {}  @{}", short_run(e.run_id), node);
            if let Some(prompt) = &e.prompt {
                let _ = writeln!(out, "      ↳ {}", first_line(prompt));
            }
            print_display_action(out, e);
            print_capacity_line(out, e);
        }
    }

    for group in [AttentionGroup::Recovery, AttentionGroup::Unknown] {
        for e in entries.iter().filter(|e| e.attention == group) {
            let _ = writeln!(out, "\n⚠ {}  {}", short_run(e.run_id), e.display.label());
            print_display_action(out, e);
        }
    }

    if !waiting.is_empty() {
        let _ = writeln!(out, "\n⏸ WAITING (parked on capacity) ({})", waiting.len());
        for e in &waiting {
            let node = e.active_node.as_deref().unwrap_or("-");
            let _ = writeln!(out, "  {}  @{}", short_run(e.run_id), node);
            let _ = writeln!(out, "      ↻ {}", format_wake_line(e));
            print_display_action(out, e);
            print_capacity_line(out, e);
        }
    }

    let _ = writeln!(out, "\n▶ WORKING ({})", working.len());
    for e in &working {
        let node = e.active_node.as_deref().unwrap_or("-");
        let _ = writeln!(out, "  {}  @{}", short_run(e.run_id), node);
        print_capacity_line(out, e);
    }

    // Spec §10/R30: a completed-but-unverified run must read differently
    // from a proven one even in the default (collapsed) view, not only
    // under `--all` — an operator skimming the count line is exactly who
    // "done ≠ done-and-proven" needs to reach.
    let unverified = done
        .iter()
        .filter(|e| e.evidence_backed == Some(false))
        .count();
    let unverified_suffix = if unverified > 0 {
        format!(", {unverified} ⚠ unverified")
    } else {
        String::new()
    };
    if show_done {
        let _ = writeln!(out, "\n✔ DONE ({}{unverified_suffix})", done.len());
        for e in &done {
            let marker = if e.evidence_backed == Some(false) {
                "  ⚠ unverified — no authorized verifier confirmed this"
            } else {
                ""
            };
            let _ = writeln!(
                out,
                "  {}  {}{marker}",
                short_run(e.run_id),
                e.done_reason.map_or("done", DoneReason::as_str)
            );
            print_capacity_line(out, e);
        }
    } else {
        let _ = writeln!(
            out,
            "\n✔ DONE: {}{unverified_suffix} (use --all to list)",
            done.len()
        );
    }
}

/// Render a parked entry's wake time and why it is what it is (Task 12 M5):
/// an operator needs to see *when* a parked run resumes on its own and
/// *why* that time was chosen — an actual provider-observed reset vs. a
/// configured blind-backoff guess — not just that the run is parked.
/// `entry.wake_at`/`wake_basis` are set together or not at all (see
/// `classify`'s `Attention::Waiting` arm), so the `None` arms below are
/// defensive, not an expected split state.
fn print_display_action(out: &mut impl std::io::Write, entry: &InboxEntry) {
    let _ = writeln!(out, "      {}", entry.display.label());
    if let Some(action) = entry.display.next_action() {
        let _ = writeln!(out, "      {action}");
    }
}

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

fn short_run(run_id: RunId) -> String {
    // The display form is `run-` + a 26-char ULID; show the last 8 characters
    // for a compact, still-unique handle in a single-user local context.
    let full = run_id.to_string();
    full[full.len().saturating_sub(8)..].to_owned()
}

fn first_line(s: &str) -> &str {
    s.lines().next().unwrap_or(s).trim()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn recovery_and_unknown_are_visible_without_all_and_explain_next_action() {
        use surge_core::run_display::{RunDisplayState, WaitingReason};
        let mut entry = done_entry(RunId::new(), None);
        entry.done_reason = None;
        entry.attention = AttentionGroup::Recovery;
        entry.display = RunDisplayState::Waiting(WaitingReason::RecoveryRequired);
        let mut unknown = done_entry(RunId::new(), None);
        unknown.done_reason = None;
        unknown.attention = AttentionGroup::Unknown;
        unknown.display = RunDisplayState::Unknown;
        let mut bytes = Vec::new();
        print_inbox_to(&mut bytes, &[entry, unknown], false);
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.contains("Recovery requires an operator"));
        assert!(text.contains("Inspect the run and choose whether to resume or abort."));
        assert!(text.contains("Run state is unconfirmed"));
        assert!(text.contains("Inspect the run journal before taking action."));
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
            run_id: RunId::new(),
            project_path: PathBuf::from("/proj"),
            display: surge_core::run_display::RunDisplayState::Waiting(
                surge_core::run_display::WaitingReason::Capacity {
                    until: wake_at,
                    basis: WakeBasis::ObservedReset,
                    runtime: None,
                },
            ),
            attention: AttentionGroup::Waiting,
            done_reason: None,
            active_node: Some("plan".into()),
            prompt: None,
            capacity: CapacityStatus::NeverObserved,
            wake_at: Some(wake_at),
            wake_basis: Some(WakeBasis::ObservedReset),
            evidence_backed: None,
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

    fn done_entry(run_id: RunId, evidence_backed: Option<bool>) -> InboxEntry {
        InboxEntry {
            run_id,
            project_path: PathBuf::from("/proj"),
            display: surge_core::run_display::RunDisplayState::Done(
                surge_core::TerminalReason::Completed,
            ),
            attention: AttentionGroup::Done,
            done_reason: Some(DoneReason::Completed),
            active_node: None,
            prompt: None,
            capacity: CapacityStatus::NeverObserved,
            wake_at: None,
            wake_basis: None,
            evidence_backed,
            started_at_ms: 0,
        }
    }

    /// Spec §10/R30, `surge inbox`'s printed (non-JSON) rendering: an
    /// unverified completed run must read differently from a verified one,
    /// in the collapsed default view (the count line) and under `--all`
    /// (the per-entry marker) alike — pinned as one test each so a mutation
    /// that only fixes one view still fails the other.
    #[test]
    fn print_inbox_flags_an_unverified_done_entry_distinctly_from_a_verified_one() {
        // `short_run` prints only the last 8 characters of the run id, so
        // the per-line assertions find each row by that printed handle.
        let (unverified_id, verified_id) = (RunId::new(), RunId::new());
        let (unverified_handle, verified_handle) =
            (short_run(unverified_id), short_run(verified_id));
        let entries = vec![
            done_entry(unverified_id, Some(false)),
            done_entry(verified_id, Some(true)),
        ];

        let collapsed = {
            let mut out = Vec::new();
            print_inbox_to(&mut out, &entries, false);
            String::from_utf8(out).unwrap()
        };
        assert!(
            collapsed.contains("1 ⚠ unverified"),
            "the collapsed DONE count line must call out the one unverified \
             success, got: {collapsed}"
        );

        let expanded = {
            let mut out = Vec::new();
            print_inbox_to(&mut out, &entries, true);
            String::from_utf8(out).unwrap()
        };
        let lines: Vec<&str> = expanded.lines().collect();
        let unverified_line = lines
            .iter()
            .find(|l| l.contains(&unverified_handle))
            .expect("unverified entry printed");
        let verified_line = lines
            .iter()
            .find(|l| l.contains(&verified_handle))
            .expect("verified entry printed");
        assert!(
            unverified_line.contains("unverified"),
            "the unverified entry's own line must carry the marker, got: {unverified_line}"
        );
        assert!(
            !verified_line.contains("unverified"),
            "a verified entry must not carry the unverified marker, got: {verified_line}"
        );
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
