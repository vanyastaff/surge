use anyhow::Result;
use clap::{Subcommand, ValueEnum};
use serde::Serialize;
use surge_persistence::{models::SessionUsage, store::Store};

use super::load_spec_by_id;

/// Output format for analytics data
#[derive(Debug, Clone, ValueEnum, Default)]
pub(crate) enum OutputFormat {
    /// Human-readable text output
    #[default]
    Text,
    /// JSON output
    Json,
    /// CSV output
    Csv,
}

/// Summary statistics for a spec
#[derive(Debug, Serialize)]
struct AnalyticsSummary {
    spec_id: Option<String>,
    total_sessions: usize,
    total_cost_usd: f64,
    input_tokens: u64,
    output_tokens: u64,
    thought_tokens: u64,
    cached_read_tokens: u64,
    cached_write_tokens: u64,
    total_tokens: u64,
}

/// Detailed session export data
#[derive(Debug, Serialize)]
struct SessionExport {
    session_id: String,
    spec_id: String,
    subtask_id: Option<String>,
    agent_name: String,
    timestamp_ms: u64,
    input_tokens: u64,
    output_tokens: u64,
    thought_tokens: u64,
    cached_read_tokens: u64,
    cached_write_tokens: u64,
    total_tokens: u64,
    estimated_cost_usd: f64,
}

#[derive(Subcommand)]
pub enum AnalyticsCommands {
    /// Show summary statistics for token usage and costs
    Summary {
        /// Spec ID or filename
        #[arg(long)]
        spec: Option<String>,

        /// Filter by agent name
        #[arg(long)]
        agent: Option<String>,

        /// Start of date range (Unix timestamp in milliseconds)
        #[arg(long)]
        from: Option<u64>,

        /// End of date range (Unix timestamp in milliseconds)
        #[arg(long)]
        to: Option<u64>,

        /// Output format
        #[arg(long, value_enum, default_value = "text")]
        format: OutputFormat,
    },

    /// Export detailed session data
    Export {
        /// Spec ID or filename
        #[arg(long)]
        spec: Option<String>,

        /// Filter by agent name
        #[arg(long)]
        agent: Option<String>,

        /// Start of date range (Unix timestamp in milliseconds)
        #[arg(long)]
        from: Option<u64>,

        /// End of date range (Unix timestamp in milliseconds)
        #[arg(long)]
        to: Option<u64>,

        /// Output format
        #[arg(long, value_enum, default_value = "json")]
        format: OutputFormat,
    },
}

pub fn run(command: AnalyticsCommands) -> Result<()> {
    match command {
        AnalyticsCommands::Summary {
            spec,
            agent,
            from,
            to,
            format,
        } => show_summary(spec, agent, from, to, format),
        AnalyticsCommands::Export {
            spec,
            agent,
            from,
            to,
            format,
        } => export_sessions(spec, agent, from, to, format),
    }
}

fn show_summary(
    spec_filter: Option<String>,
    agent_filter: Option<String>,
    from_ts: Option<u64>,
    to_ts: Option<u64>,
    format: OutputFormat,
) -> Result<()> {
    // Open the persistence store
    let store_path = Store::default_path()?;

    if !store_path.exists() {
        // For non-text formats, output empty data structure
        match format {
            OutputFormat::Json => {
                let empty_summary = AnalyticsSummary {
                    spec_id: None,
                    total_sessions: 0,
                    total_cost_usd: 0.0,
                    input_tokens: 0,
                    output_tokens: 0,
                    thought_tokens: 0,
                    cached_read_tokens: 0,
                    cached_write_tokens: 0,
                    total_tokens: 0,
                };
                println!("{}", serde_json::to_string_pretty(&empty_summary)?);
            }
            OutputFormat::Csv => {
                println!("spec_id,total_sessions,total_cost_usd,input_tokens,output_tokens,thought_tokens,cached_read_tokens,cached_write_tokens,total_tokens");
            }
            OutputFormat::Text => {
                println!("⚠️  No analytics data available yet.");
                println!(
                    "   Analytics tracking will be recorded after running specs with the orchestrator."
                );
            }
        }
        return Ok(());
    }

    let store = Store::open(&store_path)?;

    // Determine which spec(s) to query
    let (sessions, spec_id_str) = if let Some(spec_id_str) = spec_filter {
        // Load spec to get proper SpecId
        let spec_file = load_spec_by_id(&spec_id_str)?;
        let spec_id = spec_file.spec.id;

        // Get sessions for this spec
        let sessions = store.list_sessions_by_spec(spec_id)?;
        (sessions, Some(spec_id.to_string()))
    } else {
        // Get all specs and their sessions
        let all_specs = store.list_all_specs()?;
        let mut all_sessions = Vec::new();

        for spec_usage in all_specs {
            let spec_sessions = store.list_sessions_by_spec(spec_usage.spec_id)?;
            all_sessions.extend(spec_sessions);
        }

        (all_sessions, None)
    };

    // Apply filters
    let filtered_sessions: Vec<SessionUsage> = sessions
        .into_iter()
        .filter(|session| {
            // Filter by agent
            if let Some(ref agent) = agent_filter
                && &session.agent_name != agent
            {
                return false;
            }

            // Filter by date range
            if let Some(from) = from_ts
                && session.timestamp_ms < from
            {
                return false;
            }

            if let Some(to) = to_ts
                && session.timestamp_ms > to
            {
                return false;
            }

            true
        })
        .collect();

    // Calculate summary statistics
    let total_sessions = filtered_sessions.len();
    let mut total_input = 0u64;
    let mut total_output = 0u64;
    let mut total_thought = 0u64;
    let mut total_cached_read = 0u64;
    let mut total_cached_write = 0u64;
    let mut total_cost = 0.0;

    for session in &filtered_sessions {
        total_input += session.input_tokens;
        total_output += session.output_tokens;
        total_thought += session.thought_tokens.unwrap_or(0);
        total_cached_read += session.cached_read_tokens.unwrap_or(0);
        total_cached_write += session.cached_write_tokens.unwrap_or(0);
        total_cost += session.estimated_cost_usd.unwrap_or(0.0);
    }

    let total_tokens =
        total_input + total_output + total_thought + total_cached_read + total_cached_write;

    let summary = AnalyticsSummary {
        spec_id: spec_id_str,
        total_sessions,
        total_cost_usd: total_cost,
        input_tokens: total_input,
        output_tokens: total_output,
        thought_tokens: total_thought,
        cached_read_tokens: total_cached_read,
        cached_write_tokens: total_cached_write,
        total_tokens,
    };

    // Output based on format
    match format {
        OutputFormat::Text => {
            println!("📊 Analytics Summary\n");
            if let Some(spec_id) = &summary.spec_id {
                println!("Spec ID: {}", spec_id);
            } else {
                println!("Spec ID: All Specs");
            }
            println!();
            println!("Sessions: {}", summary.total_sessions);
            println!();
            println!("Token Usage:");
            println!("  Input:        {:>12}", format_number(summary.input_tokens));
            println!(
                "  Output:       {:>12}",
                format_number(summary.output_tokens)
            );
            println!(
                "  Thought:      {:>12}",
                format_number(summary.thought_tokens)
            );
            println!(
                "  Cached Read:  {:>12}",
                format_number(summary.cached_read_tokens)
            );
            println!(
                "  Cached Write: {:>12}",
                format_number(summary.cached_write_tokens)
            );
            println!(
                "  Total:        {:>12}",
                format_number(summary.total_tokens)
            );
            println!();
            println!("Estimated Cost: ${:.4}", summary.total_cost_usd);
        }
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&summary)?);
        }
        OutputFormat::Csv => {
            // Header
            println!("spec_id,total_sessions,total_cost_usd,input_tokens,output_tokens,thought_tokens,cached_read_tokens,cached_write_tokens,total_tokens");
            // Data
            println!(
                "{},{},{},{},{},{},{},{},{}",
                summary.spec_id.unwrap_or_else(|| "all".to_string()),
                summary.total_sessions,
                summary.total_cost_usd,
                summary.input_tokens,
                summary.output_tokens,
                summary.thought_tokens,
                summary.cached_read_tokens,
                summary.cached_write_tokens,
                summary.total_tokens
            );
        }
    }

    Ok(())
}

fn export_sessions(
    spec_filter: Option<String>,
    agent_filter: Option<String>,
    from_ts: Option<u64>,
    to_ts: Option<u64>,
    format: OutputFormat,
) -> Result<()> {
    // Open the persistence store
    let store_path = Store::default_path()?;

    if !store_path.exists() {
        // For non-text formats, output empty data structure
        match format {
            OutputFormat::Json => {
                println!("[]");
            }
            OutputFormat::Csv => {
                println!("session_id,spec_id,subtask_id,agent_name,timestamp_ms,input_tokens,output_tokens,thought_tokens,cached_read_tokens,cached_write_tokens,total_tokens,estimated_cost_usd");
            }
            OutputFormat::Text => {
                println!("⚠️  No session data available yet.");
            }
        }
        return Ok(());
    }

    let store = Store::open(&store_path)?;

    // Determine which spec(s) to query
    let sessions = if let Some(spec_id_str) = spec_filter {
        // Load spec to get proper SpecId
        let spec_file = load_spec_by_id(&spec_id_str)?;
        let spec_id = spec_file.spec.id;

        // Get sessions for this spec
        store.list_sessions_by_spec(spec_id)?
    } else {
        // Get all specs and their sessions
        let all_specs = store.list_all_specs()?;
        let mut all_sessions = Vec::new();

        for spec_usage in all_specs {
            let spec_sessions = store.list_sessions_by_spec(spec_usage.spec_id)?;
            all_sessions.extend(spec_sessions);
        }

        all_sessions
    };

    // Apply filters and convert to export format
    let export_data: Vec<SessionExport> = sessions
        .into_iter()
        .filter(|session| {
            // Filter by agent
            if let Some(ref agent) = agent_filter
                && &session.agent_name != agent
            {
                return false;
            }

            // Filter by date range
            if let Some(from) = from_ts
                && session.timestamp_ms < from
            {
                return false;
            }

            if let Some(to) = to_ts
                && session.timestamp_ms > to
            {
                return false;
            }

            true
        })
        .map(|session| {
            let total_tokens = session.input_tokens
                + session.output_tokens
                + session.thought_tokens.unwrap_or(0)
                + session.cached_read_tokens.unwrap_or(0)
                + session.cached_write_tokens.unwrap_or(0);

            SessionExport {
                session_id: session.session_id.to_string(),
                spec_id: session.spec_id.to_string(),
                subtask_id: session.subtask_id.map(|id| id.to_string()),
                agent_name: session.agent_name,
                timestamp_ms: session.timestamp_ms,
                input_tokens: session.input_tokens,
                output_tokens: session.output_tokens,
                thought_tokens: session.thought_tokens.unwrap_or(0),
                cached_read_tokens: session.cached_read_tokens.unwrap_or(0),
                cached_write_tokens: session.cached_write_tokens.unwrap_or(0),
                total_tokens,
                estimated_cost_usd: session.estimated_cost_usd.unwrap_or(0.0),
            }
        })
        .collect();

    // Output based on format
    match format {
        OutputFormat::Text => {
            println!("📋 Session Export ({} sessions)\n", export_data.len());
            for session in &export_data {
                println!("Session: {}", session.session_id);
                println!("  Spec:      {}", session.spec_id);
                if let Some(subtask) = &session.subtask_id {
                    println!("  Subtask:   {}", subtask);
                }
                println!("  Agent:     {}", session.agent_name);
                println!("  Timestamp: {}", session.timestamp_ms);
                println!("  Tokens:    {}", format_number(session.total_tokens));
                println!("  Cost:      ${:.4}", session.estimated_cost_usd);
                println!();
            }
        }
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&export_data)?);
        }
        OutputFormat::Csv => {
            // Header
            println!("session_id,spec_id,subtask_id,agent_name,timestamp_ms,input_tokens,output_tokens,thought_tokens,cached_read_tokens,cached_write_tokens,total_tokens,estimated_cost_usd");
            // Data
            for session in &export_data {
                println!(
                    "{},{},{},{},{},{},{},{},{},{},{},{}",
                    session.session_id,
                    session.spec_id,
                    session.subtask_id.as_deref().unwrap_or(""),
                    session.agent_name,
                    session.timestamp_ms,
                    session.input_tokens,
                    session.output_tokens,
                    session.thought_tokens,
                    session.cached_read_tokens,
                    session.cached_write_tokens,
                    session.total_tokens,
                    session.estimated_cost_usd
                );
            }
        }
    }

    Ok(())
}

/// Format a number with thousands separators
fn format_number(n: u64) -> String {
    let s = n.to_string();
    let mut result = String::new();
    let mut count = 0;

    for c in s.chars().rev() {
        if count > 0 && count % 3 == 0 {
            result.push(',');
        }
        result.push(c);
        count += 1;
    }

    result.chars().rev().collect()
}
