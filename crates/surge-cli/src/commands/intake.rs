//! `surge intake` subcommand group — currently only `list`.
//!
//! Reads `ticket_index` from the daemon's registry SQLite (`~/.surge/db/registry.sqlite`)
//! and renders a per-tracker view of pending / running / completed tickets.

use std::path::PathBuf;

use anyhow::{Context, Result, anyhow};
use clap::{Args, Subcommand};
use serde::Serialize;
use surge_persistence::intake::{IntakeRepo, IntakeRow, TicketState};

/// Subcommands for `surge intake`.
#[derive(Subcommand, Debug)]
pub enum IntakeCommand {
    /// List tracker tickets observed by the daemon, newest first.
    List(ListArgs),
}

/// `surge intake list` arguments.
#[derive(Args, Debug)]
pub struct ListArgs {
    /// Filter to a single source id (`surge.toml` source's `id`).
    #[arg(long)]
    pub tracker: Option<String>,
    /// Output format.
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    pub format: OutputFormat,
    /// Cap on returned rows. Hard ceiling: 1000.
    #[arg(long, default_value_t = 100)]
    pub limit: u32,
}

/// Available output formats for `surge intake list`.
#[derive(clap::ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    /// Pretty-printed text table (default).
    Table,
    /// JSON array, one object per row — pipeable into `jq`.
    Json,
}

/// One row in the rendered list. Stable shape for JSON consumers.
#[derive(Debug, Serialize)]
struct ListRow {
    task_id: String,
    source_id: String,
    provider: String,
    state: String,
    priority: Option<String>,
    triage_decision: Option<String>,
    run_id: Option<String>,
    first_seen: String,
    last_seen: String,
}

impl ListRow {
    fn from_intake(row: IntakeRow) -> Self {
        Self {
            task_id: row.task_id,
            source_id: row.source_id,
            provider: row.provider,
            state: state_label(row.state).into(),
            priority: row.priority,
            triage_decision: row.triage_decision,
            run_id: row.run_id,
            first_seen: row.first_seen.to_rfc3339(),
            last_seen: row.last_seen.to_rfc3339(),
        }
    }
}

/// Dispatch the `surge intake` subcommand.
///
/// # Errors
/// Returns any error from registry-DB access or output rendering.
pub fn run(command: IntakeCommand) -> Result<()> {
    match command {
        IntakeCommand::List(args) => list(args),
    }
}

fn list(args: ListArgs) -> Result<()> {
    let limit = args.limit.min(1000);
    let conn = open_registry_connection()?;
    let repo = IntakeRepo::new(&conn);
    let rows = repo
        .list_all(args.tracker.as_deref(), limit)
        .map_err(|e| anyhow!("list_all: {e}"))?
        .into_iter()
        .map(ListRow::from_intake)
        .collect::<Vec<_>>();

    match args.format {
        OutputFormat::Json => print_json(&rows)?,
        OutputFormat::Table => print_table(&rows),
    }
    Ok(())
}

fn print_json(rows: &[ListRow]) -> Result<()> {
    let json = serde_json::to_string_pretty(rows).context("serialize intake list to JSON")?;
    println!("{json}");
    Ok(())
}

fn print_table(rows: &[ListRow]) {
    if rows.is_empty() {
        println!("(no tickets in the registry yet)");
        return;
    }

    let widths = ColumnWidths::compute(rows);
    print_table_header(&widths);
    for row in rows {
        print_table_row(row, &widths);
    }
    println!("\n{} ticket(s)", rows.len());
}

struct ColumnWidths {
    source: usize,
    task: usize,
    state: usize,
    priority: usize,
    run: usize,
    last_seen: usize,
}

impl ColumnWidths {
    fn compute(rows: &[ListRow]) -> Self {
        let mut widths = Self {
            source: "SOURCE".len(),
            task: "TASK".len(),
            state: "STATE".len(),
            priority: "PRIO".len(),
            run: "RUN".len(),
            last_seen: "LAST SEEN".len(),
        };
        for r in rows {
            widths.source = widths.source.max(r.source_id.len());
            widths.task = widths.task.max(r.task_id.len());
            widths.state = widths.state.max(r.state.len());
            widths.priority = widths
                .priority
                .max(r.priority.as_deref().unwrap_or("-").len());
            widths.run = widths.run.max(r.run_id.as_deref().unwrap_or("-").len());
            widths.last_seen = widths.last_seen.max(r.last_seen.len());
        }
        widths
    }
}

fn print_table_header(w: &ColumnWidths) {
    println!(
        "{:<src$}  {:<task$}  {:<state$}  {:<prio$}  {:<run$}  {:<last$}",
        "SOURCE",
        "TASK",
        "STATE",
        "PRIO",
        "RUN",
        "LAST SEEN",
        src = w.source,
        task = w.task,
        state = w.state,
        prio = w.priority,
        run = w.run,
        last = w.last_seen,
    );
}

fn print_table_row(row: &ListRow, w: &ColumnWidths) {
    println!(
        "{:<src$}  {:<task$}  {:<state$}  {:<prio$}  {:<run$}  {:<last$}",
        row.source_id,
        row.task_id,
        row.state,
        row.priority.as_deref().unwrap_or("-"),
        row.run_id.as_deref().unwrap_or("-"),
        row.last_seen,
        src = w.source,
        task = w.task,
        state = w.state,
        prio = w.priority,
        run = w.run,
        last = w.last_seen,
    );
}

/// Stable on-disk string for a [`TicketState`]. Exposes the same label
/// the FSM persists, with one cosmetic remap so `L0Skipped` rows show
/// up readably in the CLI.
fn state_label(state: TicketState) -> &'static str {
    state.as_str()
}

fn open_registry_connection() -> Result<rusqlite::Connection> {
    let home = surge_home_dir()?;
    let db_path = home.join("db").join("registry.sqlite");
    if !db_path.exists() {
        return Err(anyhow!(
            "registry database not found at {} — start the daemon at least once",
            db_path.display()
        ));
    }
    rusqlite::Connection::open(&db_path).map_err(|e| anyhow!("open registry: {e}"))
}

fn surge_home_dir() -> Result<PathBuf> {
    if let Ok(custom) = std::env::var("SURGE_HOME")
        && !custom.is_empty()
    {
        return Ok(PathBuf::from(custom));
    }
    let base = dirs::home_dir().ok_or_else(|| anyhow!("could not resolve home directory"))?;
    Ok(base.join(".surge"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use surge_persistence::runs::clock::MockClock;
    use surge_persistence::runs::migrations::{REGISTRY_MIGRATIONS, apply};

    fn db_with_full_registry() -> rusqlite::Connection {
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        let clock = MockClock::new(1_700_000_000_000);
        apply(&mut conn, REGISTRY_MIGRATIONS, &clock).unwrap();
        conn
    }

    fn sample_intake_row(task_id: &str, state: TicketState) -> IntakeRow {
        IntakeRow {
            task_id: task_id.into(),
            source_id: "github_issues:user/repo".into(),
            provider: "github_issues".into(),
            run_id: None,
            triage_decision: Some("Enqueued".into()),
            duplicate_of: None,
            priority: Some("medium".into()),
            state,
            first_seen: chrono::Utc::now(),
            last_seen: chrono::Utc::now(),
            snooze_until: None,
            callback_token: None,
            tg_chat_id: None,
            tg_message_id: None,
        }
    }

    #[test]
    fn list_row_serializes_with_stable_field_names() {
        let intake = sample_intake_row("github_issues:user/repo#42", TicketState::Active);
        let row = ListRow::from_intake(intake);
        let json = serde_json::to_value(&row).unwrap();
        assert_eq!(json["task_id"], "github_issues:user/repo#42");
        assert_eq!(json["state"], "Active");
        assert_eq!(json["priority"], "medium");
        assert_eq!(json["triage_decision"], "Enqueued");
        assert!(json["first_seen"].is_string());
    }

    #[test]
    fn list_row_roundtrips_through_intake_repo() {
        let conn = db_with_full_registry();
        let repo = IntakeRepo::new(&conn);
        repo.insert(&sample_intake_row(
            "github_issues:org/repo#7",
            TicketState::RunStarted,
        ))
        .unwrap();
        let rows = repo.list_all(None, 50).unwrap();
        assert_eq!(rows.len(), 1);
        let view = ListRow::from_intake(rows.into_iter().next().unwrap());
        assert_eq!(view.task_id, "github_issues:org/repo#7");
        assert_eq!(view.state, "RunStarted");
    }

    #[test]
    fn column_widths_grow_for_long_values() {
        let mut rows = Vec::new();
        let mut r = sample_intake_row("github_issues:org/repo#1", TicketState::Active);
        r.run_id = Some("01H".repeat(10));
        rows.push(ListRow::from_intake(r));
        let w = ColumnWidths::compute(&rows);
        assert!(w.run >= 30);
    }

    #[test]
    fn json_format_renders_array() {
        // Render a small slice to exercise the JSON path.
        let intake = sample_intake_row("linear:wsp/ABC-1", TicketState::Seen);
        let rows = vec![ListRow::from_intake(intake)];
        let json = serde_json::to_string(&rows).unwrap();
        assert!(json.starts_with('['));
        assert!(json.ends_with(']'));
        assert!(json.contains("\"task_id\":\"linear:wsp/ABC-1\""));
    }
}
