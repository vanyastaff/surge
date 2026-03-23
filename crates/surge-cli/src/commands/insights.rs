use anyhow::Result;
use clap::{Subcommand, ValueEnum};
use serde::Serialize;
use surge_persistence::{models::SessionUsage, store::Store};

use super::load_spec_by_id;

/// Output format for insights data
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

/// Aggregated subtask cost data for export
#[derive(Debug, Serialize)]
struct SubtaskCostData {
    subtask_id: String,
    session_count: usize,
    input_tokens: u64,
    output_tokens: u64,
    thought_tokens: u64,
    total_tokens: u64,
    estimated_cost_usd: f64,
}

/// Summary data for export
#[derive(Debug, Serialize)]
struct SummaryData {
    total_sessions: usize,
    input_tokens: u64,
    output_tokens: u64,
    thought_tokens: u64,
    cached_read_tokens: u64,
    cached_write_tokens: u64,
    total_tokens: u64,
    total_cost_usd: f64,
}

/// Complete cost insights export data
#[derive(Debug, Serialize)]
struct CostInsights {
    subtasks: Vec<SubtaskCostData>,
    sessions_without_subtask: Option<SubtaskCostData>,
    summary: SummaryData,
}

#[derive(Subcommand)]
pub enum InsightsCommands {
    /// Show cost breakdown by subtask
    Cost {
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
}

pub fn run(command: InsightsCommands) -> Result<()> {
    match command {
        InsightsCommands::Cost { spec, agent, from, to, format } => {
            show_cost(spec, agent, from, to, format)
        }
    }
}

fn show_cost(
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
                let empty_insights = CostInsights {
                    subtasks: vec![],
                    sessions_without_subtask: None,
                    summary: SummaryData {
                        total_sessions: 0,
                        input_tokens: 0,
                        output_tokens: 0,
                        thought_tokens: 0,
                        cached_read_tokens: 0,
                        cached_write_tokens: 0,
                        total_tokens: 0,
                        total_cost_usd: 0.0,
                    },
                };
                println!("{}", serde_json::to_string_pretty(&empty_insights)?);
            }
            OutputFormat::Csv => {
                // CSV header only
                println!("subtask_id,session_count,input_tokens,output_tokens,thought_tokens,total_tokens,estimated_cost_usd");
            }
            OutputFormat::Text => {
                println!("⚠️  No cost data available yet.");
                println!("   Cost tracking will be recorded after running specs with the orchestrator.");
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

        if matches!(format, OutputFormat::Text) {
            println!("⚡ Cost Insights: {}\n", spec_file.spec.title);
            println!("ID: {}", spec_id);
        }

        // Get sessions for this spec
        store.list_sessions_by_spec(spec_id)?
    } else {
        if matches!(format, OutputFormat::Text) {
            println!("⚡ Cost Insights: All Specs\n");
        }

        // Get all specs and their sessions
        let all_specs = store.list_all_specs()?;
        let mut all_sessions = Vec::new();

        for spec_usage in all_specs {
            let spec_sessions = store.list_sessions_by_spec(spec_usage.spec_id)?;
            all_sessions.extend(spec_sessions);
        }

        all_sessions
    };

    // Apply filters
    let filtered_sessions: Vec<SessionUsage> = sessions
        .into_iter()
        .filter(|session| {
            // Filter by agent
            if let Some(ref agent) = agent_filter {
                if &session.agent_name != agent {
                    return false;
                }
            }

            // Filter by date range
            if let Some(from) = from_ts {
                if session.timestamp_ms < from {
                    return false;
                }
            }

            if let Some(to) = to_ts {
                if session.timestamp_ms > to {
                    return false;
                }
            }

            true
        })
        .collect();

    if filtered_sessions.is_empty() {
        // For non-text formats, output empty data structure
        match format {
            OutputFormat::Json => {
                let empty_insights = CostInsights {
                    subtasks: vec![],
                    sessions_without_subtask: None,
                    summary: SummaryData {
                        total_sessions: 0,
                        input_tokens: 0,
                        output_tokens: 0,
                        thought_tokens: 0,
                        cached_read_tokens: 0,
                        cached_write_tokens: 0,
                        total_tokens: 0,
                        total_cost_usd: 0.0,
                    },
                };
                println!("{}", serde_json::to_string_pretty(&empty_insights)?);
            }
            OutputFormat::Csv => {
                // CSV header only
                println!("subtask_id,session_count,input_tokens,output_tokens,thought_tokens,total_tokens,estimated_cost_usd");
            }
            OutputFormat::Text => {
                println!("\n⚠️  No sessions found matching the specified filters.");
            }
        }
        return Ok(());
    }

    // Display filter info (text format only)
    if matches!(format, OutputFormat::Text) && (agent_filter.is_some() || from_ts.is_some() || to_ts.is_some()) {
        println!("\nFilters applied:");
        if let Some(ref agent) = agent_filter {
            println!("   Agent: {}", agent);
        }
        if let Some(from) = from_ts {
            println!("   From: {} (Unix ms)", from);
        }
        if let Some(to) = to_ts {
            println!("   To: {} (Unix ms)", to);
        }
    }

    // Aggregate by subtask
    let mut subtask_map: std::collections::HashMap<String, Vec<&SessionUsage>> =
        std::collections::HashMap::new();
    let mut sessions_without_subtask = Vec::new();

    for session in &filtered_sessions {
        if let Some(ref subtask_id) = session.subtask_id {
            let subtask_id_str = subtask_id.to_string();
            subtask_map
                .entry(subtask_id_str)
                .or_default()
                .push(session);
        } else {
            sessions_without_subtask.push(session);
        }
    }

    // Prepare aggregated data
    let mut subtask_entries: Vec<_> = subtask_map.iter().collect();
    // Sort by cost descending (manually since f64 doesn't implement Ord)
    subtask_entries.sort_by(|(_, sessions_a), (_, sessions_b)| {
        let cost_a: f64 = sessions_a
            .iter()
            .map(|s| s.estimated_cost_usd.unwrap_or(0.0))
            .sum();
        let cost_b: f64 = sessions_b
            .iter()
            .map(|s| s.estimated_cost_usd.unwrap_or(0.0))
            .sum();
        cost_b.partial_cmp(&cost_a).unwrap_or(std::cmp::Ordering::Equal)
    });

    // Calculate summary totals
    let total_input: u64 = filtered_sessions.iter().map(|s| s.input_tokens).sum();
    let total_output: u64 = filtered_sessions.iter().map(|s| s.output_tokens).sum();
    let total_thought: u64 = filtered_sessions
        .iter()
        .map(|s| s.thought_tokens.unwrap_or(0))
        .sum();
    let total_cached_read: u64 = filtered_sessions
        .iter()
        .map(|s| s.cached_read_tokens.unwrap_or(0))
        .sum();
    let total_cached_write: u64 = filtered_sessions
        .iter()
        .map(|s| s.cached_write_tokens.unwrap_or(0))
        .sum();
    let total_cost: f64 = filtered_sessions
        .iter()
        .map(|s| s.estimated_cost_usd.unwrap_or(0.0))
        .sum();

    // Output based on format
    match format {
        OutputFormat::Json => {
            output_json(&subtask_entries, &sessions_without_subtask, &filtered_sessions,
                total_input, total_output, total_thought, total_cached_read, total_cached_write, total_cost)?;
        }
        OutputFormat::Csv => {
            output_csv(&subtask_entries, &sessions_without_subtask)?;
        }
        OutputFormat::Text => {
            output_text(&subtask_entries, &sessions_without_subtask, &filtered_sessions,
                total_input, total_output, total_thought, total_cached_read, total_cached_write, total_cost);
        }
    }

    Ok(())
}

/// Output cost insights in JSON format
fn output_json(
    subtask_entries: &[(&String, &Vec<&SessionUsage>)],
    sessions_without_subtask: &[&SessionUsage],
    filtered_sessions: &[SessionUsage],
    total_input: u64,
    total_output: u64,
    total_thought: u64,
    total_cached_read: u64,
    total_cached_write: u64,
    total_cost: f64,
) -> Result<()> {
    let mut subtasks_data = Vec::new();

    // Add subtask data
    for (subtask_id, sessions) in subtask_entries {
        let input: u64 = sessions.iter().map(|s| s.input_tokens).sum();
        let output: u64 = sessions.iter().map(|s| s.output_tokens).sum();
        let thought: u64 = sessions.iter().map(|s| s.thought_tokens.unwrap_or(0)).sum();
        let cost: f64 = sessions.iter().map(|s| s.estimated_cost_usd.unwrap_or(0.0)).sum();

        subtasks_data.push(SubtaskCostData {
            subtask_id: (*subtask_id).clone(),
            session_count: sessions.len(),
            input_tokens: input,
            output_tokens: output,
            thought_tokens: thought,
            total_tokens: input + output + thought,
            estimated_cost_usd: cost,
        });
    }

    // Sessions without subtask
    let sessions_no_subtask = if !sessions_without_subtask.is_empty() {
        let input: u64 = sessions_without_subtask.iter().map(|s| s.input_tokens).sum();
        let output: u64 = sessions_without_subtask.iter().map(|s| s.output_tokens).sum();
        let thought: u64 = sessions_without_subtask.iter().map(|s| s.thought_tokens.unwrap_or(0)).sum();
        let cost: f64 = sessions_without_subtask.iter().map(|s| s.estimated_cost_usd.unwrap_or(0.0)).sum();

        Some(SubtaskCostData {
            subtask_id: "(no subtask)".to_string(),
            session_count: sessions_without_subtask.len(),
            input_tokens: input,
            output_tokens: output,
            thought_tokens: thought,
            total_tokens: input + output + thought,
            estimated_cost_usd: cost,
        })
    } else {
        None
    };

    let insights = CostInsights {
        subtasks: subtasks_data,
        sessions_without_subtask: sessions_no_subtask,
        summary: SummaryData {
            total_sessions: filtered_sessions.len(),
            input_tokens: total_input,
            output_tokens: total_output,
            thought_tokens: total_thought,
            cached_read_tokens: total_cached_read,
            cached_write_tokens: total_cached_write,
            total_tokens: total_input + total_output + total_thought,
            total_cost_usd: total_cost,
        },
    };

    println!("{}", serde_json::to_string_pretty(&insights)?);
    Ok(())
}

/// Output cost insights in CSV format
fn output_csv(
    subtask_entries: &[(&String, &Vec<&SessionUsage>)],
    sessions_without_subtask: &[&SessionUsage],
) -> Result<()> {
    // CSV header
    println!("subtask_id,session_count,input_tokens,output_tokens,thought_tokens,total_tokens,estimated_cost_usd");

    // Subtask rows
    for (subtask_id, sessions) in subtask_entries {
        let input: u64 = sessions.iter().map(|s| s.input_tokens).sum();
        let output: u64 = sessions.iter().map(|s| s.output_tokens).sum();
        let thought: u64 = sessions.iter().map(|s| s.thought_tokens.unwrap_or(0)).sum();
        let cost: f64 = sessions.iter().map(|s| s.estimated_cost_usd.unwrap_or(0.0)).sum();
        let total = input + output + thought;

        println!("{},{},{},{},{},{},{:.4}", subtask_id, sessions.len(), input, output, thought, total, cost);
    }

    // Sessions without subtask
    if !sessions_without_subtask.is_empty() {
        let input: u64 = sessions_without_subtask.iter().map(|s| s.input_tokens).sum();
        let output: u64 = sessions_without_subtask.iter().map(|s| s.output_tokens).sum();
        let thought: u64 = sessions_without_subtask.iter().map(|s| s.thought_tokens.unwrap_or(0)).sum();
        let cost: f64 = sessions_without_subtask.iter().map(|s| s.estimated_cost_usd.unwrap_or(0.0)).sum();
        let total = input + output + thought;

        println!("(no subtask),{},{},{},{},{},{:.4}", sessions_without_subtask.len(), input, output, thought, total, cost);
    }

    Ok(())
}

/// Output cost insights in text format
fn output_text(
    subtask_entries: &[(&String, &Vec<&SessionUsage>)],
    sessions_without_subtask: &[&SessionUsage],
    filtered_sessions: &[SessionUsage],
    total_input: u64,
    total_output: u64,
    total_thought: u64,
    total_cached_read: u64,
    total_cached_write: u64,
    total_cost: f64,
) {
    // Display subtask breakdown
    if !subtask_entries.is_empty() {
        println!("\n📊 Subtask Breakdown:");

        for (subtask_id, sessions) in subtask_entries {
            let input: u64 = sessions.iter().map(|s| s.input_tokens).sum();
            let output: u64 = sessions.iter().map(|s| s.output_tokens).sum();
            let thought: u64 = sessions.iter().map(|s| s.thought_tokens.unwrap_or(0)).sum();
            let cost: f64 = sessions.iter().map(|s| s.estimated_cost_usd.unwrap_or(0.0)).sum();

            println!("\n   Subtask: {}", subtask_id);
            println!("      Sessions: {}", sessions.len());
            println!("      Input tokens: {}", format_tokens(input));
            println!("      Output tokens: {}", format_tokens(output));
            if thought > 0 {
                println!("      Thought tokens: {}", format_tokens(thought));
            }
            println!("      Total tokens: {}", format_tokens(input + output + thought));
            println!("      Estimated cost: ${:.4}", cost);
        }
    }

    // Display sessions without subtask
    if !sessions_without_subtask.is_empty() {
        println!("\n📊 Sessions without subtask:");
        let input: u64 = sessions_without_subtask.iter().map(|s| s.input_tokens).sum();
        let output: u64 = sessions_without_subtask.iter().map(|s| s.output_tokens).sum();
        let thought: u64 = sessions_without_subtask.iter().map(|s| s.thought_tokens.unwrap_or(0)).sum();
        let cost: f64 = sessions_without_subtask.iter().map(|s| s.estimated_cost_usd.unwrap_or(0.0)).sum();

        println!("   Sessions: {}", sessions_without_subtask.len());
        println!("   Input tokens: {}", format_tokens(input));
        println!("   Output tokens: {}", format_tokens(output));
        if thought > 0 {
            println!("   Thought tokens: {}", format_tokens(thought));
        }
        println!("   Total tokens: {}", format_tokens(input + output + thought));
        println!("   Estimated cost: ${:.4}", cost);
    }

    // Overall summary
    println!("\n📈 Summary:");
    println!("   Total sessions: {}", filtered_sessions.len());
    println!("   Input tokens: {}", format_tokens(total_input));
    println!("   Output tokens: {}", format_tokens(total_output));
    if total_thought > 0 {
        println!("   Thought tokens: {}", format_tokens(total_thought));
    }
    if total_cached_read > 0 {
        println!("   Cached read tokens: {}", format_tokens(total_cached_read));
    }
    if total_cached_write > 0 {
        println!("   Cached write tokens: {}", format_tokens(total_cached_write));
    }
    println!("   Total tokens: {}", format_tokens(total_input + total_output + total_thought));
    println!("   Total cost: ${:.4}", total_cost);
}

/// Format token count with thousands separator
fn format_tokens(tokens: u64) -> String {
    let s = tokens.to_string();
    let mut result = String::new();
    let mut count = 0;

    for c in s.chars().rev() {
        if count == 3 {
            result.push(',');
            count = 0;
        }
        result.push(c);
        count += 1;
    }

    result.chars().rev().collect()
}
