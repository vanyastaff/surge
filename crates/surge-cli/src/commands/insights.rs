use anyhow::Result;
use clap::Subcommand;
use surge_persistence::{models::SessionUsage, store::Store};

use super::load_spec_by_id;

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
    },
}

pub fn run(command: InsightsCommands) -> Result<()> {
    match command {
        InsightsCommands::Cost { spec, agent, from, to } => {
            show_cost(spec, agent, from, to)
        }
    }
}

fn show_cost(
    spec_filter: Option<String>,
    agent_filter: Option<String>,
    from_ts: Option<u64>,
    to_ts: Option<u64>,
) -> Result<()> {
    // Open the persistence store
    let store_path = Store::default_path()?;

    if !store_path.exists() {
        println!("⚠️  No cost data available yet.");
        println!("   Cost tracking will be recorded after running specs with the orchestrator.");
        return Ok(());
    }

    let store = Store::open(&store_path)?;

    // Determine which spec(s) to query
    let sessions = if let Some(spec_id_str) = spec_filter {
        // Load spec to get proper SpecId
        let spec_file = load_spec_by_id(&spec_id_str)?;
        let spec_id = spec_file.spec.id;

        println!("⚡ Cost Insights: {}\n", spec_file.spec.title);
        println!("ID: {}", spec_id);

        // Get sessions for this spec
        store.list_sessions_by_spec(spec_id)?
    } else {
        println!("⚡ Cost Insights: All Specs\n");

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
        println!("\n⚠️  No sessions found matching the specified filters.");
        return Ok(());
    }

    // Display filter info
    if agent_filter.is_some() || from_ts.is_some() || to_ts.is_some() {
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

    // Display subtask breakdown
    if !subtask_map.is_empty() {
        println!("\n📊 Subtask Breakdown:");

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

        for (subtask_id, sessions) in subtask_entries {
            let total_input: u64 = sessions.iter().map(|s| s.input_tokens).sum();
            let total_output: u64 = sessions.iter().map(|s| s.output_tokens).sum();
            let total_thought: u64 = sessions
                .iter()
                .map(|s| s.thought_tokens.unwrap_or(0))
                .sum();
            let total_cost: f64 = sessions
                .iter()
                .map(|s| s.estimated_cost_usd.unwrap_or(0.0))
                .sum();
            let session_count = sessions.len();

            println!("\n   Subtask: {}", subtask_id);
            println!("      Sessions: {}", session_count);
            println!("      Input tokens: {}", format_tokens(total_input));
            println!("      Output tokens: {}", format_tokens(total_output));
            if total_thought > 0 {
                println!("      Thought tokens: {}", format_tokens(total_thought));
            }
            println!(
                "      Total tokens: {}",
                format_tokens(total_input + total_output + total_thought)
            );
            println!("      Estimated cost: ${:.4}", total_cost);
        }
    }

    // Display sessions without subtask
    if !sessions_without_subtask.is_empty() {
        println!("\n📊 Sessions without subtask:");
        let total_input: u64 = sessions_without_subtask.iter().map(|s| s.input_tokens).sum();
        let total_output: u64 = sessions_without_subtask
            .iter()
            .map(|s| s.output_tokens)
            .sum();
        let total_thought: u64 = sessions_without_subtask
            .iter()
            .map(|s| s.thought_tokens.unwrap_or(0))
            .sum();
        let total_cost: f64 = sessions_without_subtask
            .iter()
            .map(|s| s.estimated_cost_usd.unwrap_or(0.0))
            .sum();

        println!("   Sessions: {}", sessions_without_subtask.len());
        println!("   Input tokens: {}", format_tokens(total_input));
        println!("   Output tokens: {}", format_tokens(total_output));
        if total_thought > 0 {
            println!("   Thought tokens: {}", format_tokens(total_thought));
        }
        println!(
            "   Total tokens: {}",
            format_tokens(total_input + total_output + total_thought)
        );
        println!("   Estimated cost: ${:.4}", total_cost);
    }

    // Overall summary
    println!("\n📈 Summary:");
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
    println!(
        "   Total tokens: {}",
        format_tokens(total_input + total_output + total_thought)
    );
    println!("   Total cost: ${:.4}", total_cost);

    Ok(())
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
