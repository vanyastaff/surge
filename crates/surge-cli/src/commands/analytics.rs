use anyhow::Result;
use clap::{Subcommand, ValueEnum};
use serde::Serialize;
use surge_core::config::SurgeConfig;
use surge_persistence::{budget::BudgetTracker, models::SessionUsage, store::Store};

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
    #[serde(skip_serializing_if = "Option::is_none")]
    top_tasks: Option<Vec<TaskCost>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    agent_breakdown: Option<Vec<AgentCost>>,
}

/// Cost data for a task (spec)
#[derive(Debug, Serialize)]
struct TaskCost {
    spec_id: String,
    session_count: usize,
    total_cost_usd: f64,
    total_tokens: u64,
}

/// Cost data for an agent
#[derive(Debug, Serialize)]
struct AgentCost {
    agent_name: String,
    session_count: usize,
    total_cost_usd: f64,
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

/// Budget status export data
#[derive(Debug, Serialize)]
struct BudgetStatusExport {
    daily_budget_usd: Option<f64>,
    daily_spending_usd: f64,
    daily_usage_percentage: f64,
    daily_warning_level: String,
    daily_remaining_usd: f64,
    weekly_budget_usd: Option<f64>,
    weekly_spending_usd: f64,
    weekly_usage_percentage: f64,
    weekly_warning_level: String,
    weekly_remaining_usd: f64,
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

    /// Check budget status with threshold warnings
    BudgetStatus {
        /// Output format
        #[arg(long, value_enum, default_value = "text")]
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
        AnalyticsCommands::BudgetStatus { format } => show_budget_status(format),
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
                    top_tasks: None,
                    agent_breakdown: None,
                };
                println!("{}", serde_json::to_string_pretty(&empty_summary)?);
            },
            OutputFormat::Csv => {
                println!(
                    "spec_id,total_sessions,total_cost_usd,input_tokens,output_tokens,thought_tokens,cached_read_tokens,cached_write_tokens,total_tokens"
                );
            },
            OutputFormat::Text => {
                println!("⚠️  No analytics data available yet.");
                println!(
                    "   Analytics tracking will be recorded after running specs with the orchestrator."
                );
            },
        }
        return Ok(());
    }

    let store = Store::open(&store_path)?;

    // Default to "this week" if no date range specified
    let (from_ts, to_ts, is_this_week) = if from_ts.is_none() && to_ts.is_none() {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        let week_ago_ms = now_ms - (7 * 24 * 60 * 60 * 1000); // 7 days in milliseconds
        (Some(week_ago_ms), Some(now_ms), true)
    } else {
        (from_ts, to_ts, false)
    };

    // Determine which spec(s) to query
    let (sessions, spec_id_str, has_spec_filter) = if let Some(ref spec_id_str) = spec_filter {
        // Load spec to get proper SpecId
        let spec_file = load_spec_by_id(spec_id_str)?;
        let spec_id = spec_file.spec.id;

        // Get sessions for this spec
        let sessions = store.list_sessions_by_spec(spec_id)?;
        (sessions, Some(spec_id.to_string()), true)
    } else {
        // Get all specs and their sessions
        let all_specs = store.list_all_specs()?;
        let mut all_sessions = Vec::new();

        for spec_usage in all_specs {
            let spec_sessions = store.list_sessions_by_spec(spec_usage.spec_id)?;
            all_sessions.extend(spec_sessions);
        }

        (all_sessions, None, false)
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

    // Aggregate by task (spec_id) for top tasks - only when not filtering by spec
    let top_tasks = if !has_spec_filter {
        let mut task_map: std::collections::HashMap<String, Vec<&SessionUsage>> =
            std::collections::HashMap::new();

        for session in &filtered_sessions {
            let spec_id = session.spec_id.to_string();
            task_map.entry(spec_id).or_default().push(session);
        }

        let mut task_costs: Vec<TaskCost> = task_map
            .into_iter()
            .map(|(spec_id, sessions)| {
                let session_count = sessions.len();
                let cost: f64 = sessions
                    .iter()
                    .map(|s| s.estimated_cost_usd.unwrap_or(0.0))
                    .sum();
                let tokens: u64 = sessions
                    .iter()
                    .map(|s| {
                        s.input_tokens
                            + s.output_tokens
                            + s.thought_tokens.unwrap_or(0)
                            + s.cached_read_tokens.unwrap_or(0)
                            + s.cached_write_tokens.unwrap_or(0)
                    })
                    .sum();

                TaskCost {
                    spec_id,
                    session_count,
                    total_cost_usd: cost,
                    total_tokens: tokens,
                }
            })
            .collect();

        // Sort by cost descending
        task_costs.sort_by(|a, b| {
            b.total_cost_usd
                .partial_cmp(&a.total_cost_usd)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Take top 3
        task_costs.truncate(3);
        Some(task_costs)
    } else {
        None
    };

    // Aggregate by agent for per-agent breakdown
    let agent_breakdown = if agent_filter.is_none() {
        let mut agent_map: std::collections::HashMap<String, Vec<&SessionUsage>> =
            std::collections::HashMap::new();

        for session in &filtered_sessions {
            let agent_name = session.agent_name.clone();
            agent_map.entry(agent_name).or_default().push(session);
        }

        let mut agent_costs: Vec<AgentCost> = agent_map
            .into_iter()
            .map(|(agent_name, sessions)| {
                let session_count = sessions.len();
                let cost: f64 = sessions
                    .iter()
                    .map(|s| s.estimated_cost_usd.unwrap_or(0.0))
                    .sum();
                let tokens: u64 = sessions
                    .iter()
                    .map(|s| {
                        s.input_tokens
                            + s.output_tokens
                            + s.thought_tokens.unwrap_or(0)
                            + s.cached_read_tokens.unwrap_or(0)
                            + s.cached_write_tokens.unwrap_or(0)
                    })
                    .sum();

                AgentCost {
                    agent_name,
                    session_count,
                    total_cost_usd: cost,
                    total_tokens: tokens,
                }
            })
            .collect();

        // Sort by cost descending
        agent_costs.sort_by(|a, b| {
            b.total_cost_usd
                .partial_cmp(&a.total_cost_usd)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        Some(agent_costs)
    } else {
        None
    };

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
        top_tasks,
        agent_breakdown,
    };

    // Output based on format
    match format {
        OutputFormat::Text => {
            if is_this_week {
                println!("📊 Analytics Summary - This Week\n");
            } else {
                println!("📊 Analytics Summary\n");
            }

            if let Some(spec_id) = &summary.spec_id {
                println!("Spec ID: {}", spec_id);
            } else {
                println!("Spec ID: All Specs");
            }
            println!();
            println!("Sessions: {}", summary.total_sessions);
            println!();
            println!("💰 Total Cost This Week: ${:.4}", summary.total_cost_usd);
            println!();
            println!("Token Usage:");
            println!(
                "  Input:        {:>12}",
                super::format::format_number(summary.input_tokens)
            );
            println!(
                "  Output:       {:>12}",
                super::format::format_number(summary.output_tokens)
            );
            println!(
                "  Thought:      {:>12}",
                super::format::format_number(summary.thought_tokens)
            );
            println!(
                "  Cached Read:  {:>12}",
                super::format::format_number(summary.cached_read_tokens)
            );
            println!(
                "  Cached Write: {:>12}",
                super::format::format_number(summary.cached_write_tokens)
            );
            println!(
                "  Total:        {:>12}",
                super::format::format_number(summary.total_tokens)
            );

            // Display top 3 tasks
            if let Some(ref top_tasks) = summary.top_tasks
                && !top_tasks.is_empty()
            {
                println!();
                println!("🏆 Top 3 Costliest Tasks:");
                for (i, task) in top_tasks.iter().enumerate() {
                    println!();
                    println!("   {}. {}", i + 1, task.spec_id);
                    println!("      Sessions: {}", task.session_count);
                    println!(
                        "      Tokens:   {}",
                        super::format::format_number(task.total_tokens)
                    );
                    println!("      Cost:     ${:.4}", task.total_cost_usd);
                }
            }

            // Display per-agent breakdown
            if let Some(ref agents) = summary.agent_breakdown
                && !agents.is_empty()
            {
                println!();
                println!("🤖 Per-Agent Breakdown:");
                for agent in agents {
                    println!();
                    println!("   Agent: {}", agent.agent_name);
                    println!("      Sessions: {}", agent.session_count);
                    println!(
                        "      Tokens:   {}",
                        super::format::format_number(agent.total_tokens)
                    );
                    println!("      Cost:     ${:.4}", agent.total_cost_usd);
                }
            }
        },
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&summary)?);
        },
        OutputFormat::Csv => {
            // Header
            println!(
                "spec_id,total_sessions,total_cost_usd,input_tokens,output_tokens,thought_tokens,cached_read_tokens,cached_write_tokens,total_tokens"
            );
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
        },
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
            },
            OutputFormat::Csv => {
                println!(
                    "session_id,spec_id,subtask_id,agent_name,timestamp_ms,input_tokens,output_tokens,thought_tokens,cached_read_tokens,cached_write_tokens,total_tokens,estimated_cost_usd"
                );
            },
            OutputFormat::Text => {
                println!("⚠️  No session data available yet.");
            },
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
                println!(
                    "  Tokens:    {}",
                    super::format::format_number(session.total_tokens)
                );
                println!("  Cost:      ${:.4}", session.estimated_cost_usd);
                println!();
            }
        },
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&export_data)?);
        },
        OutputFormat::Csv => {
            // Header
            println!(
                "session_id,spec_id,subtask_id,agent_name,timestamp_ms,input_tokens,output_tokens,thought_tokens,cached_read_tokens,cached_write_tokens,total_tokens,estimated_cost_usd"
            );
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
        },
    }

    Ok(())
}

fn show_budget_status(format: OutputFormat) -> Result<()> {
    // Load config to get budget settings
    let mut config = SurgeConfig::load_or_default()?;
    config.apply_env_overrides();

    let daily_budget_usd = config.analytics.budget_usd;
    let weekly_budget_usd = config.analytics.budget_usd.map(|d| d * 7.0);
    let warn_threshold = config.analytics.budget_warn_threshold;

    // If no budget configured, inform the user
    if daily_budget_usd.is_none() {
        match format {
            OutputFormat::Json => {
                let export = BudgetStatusExport {
                    daily_budget_usd: None,
                    daily_spending_usd: 0.0,
                    daily_usage_percentage: 0.0,
                    daily_warning_level: "none".to_string(),
                    daily_remaining_usd: 0.0,
                    weekly_budget_usd: None,
                    weekly_spending_usd: 0.0,
                    weekly_usage_percentage: 0.0,
                    weekly_warning_level: "none".to_string(),
                    weekly_remaining_usd: 0.0,
                };
                println!("{}", serde_json::to_string_pretty(&export)?);
            },
            OutputFormat::Csv => {
                println!(
                    "period,budget_usd,spending_usd,usage_percentage,warning_level,remaining_usd"
                );
            },
            OutputFormat::Text => {
                println!("💰 Budget Status\n");
                println!("⚠️  No budget configured.");
                println!(
                    "   Set budget_usd in [analytics] section of surge.toml to enable budget tracking."
                );
            },
        }
        return Ok(());
    }

    // Open the persistence store
    let store_path = Store::default_path()?;

    if !store_path.exists() {
        match format {
            OutputFormat::Json => {
                let export = BudgetStatusExport {
                    daily_budget_usd,
                    daily_spending_usd: 0.0,
                    daily_usage_percentage: 0.0,
                    daily_warning_level: "ok".to_string(),
                    daily_remaining_usd: daily_budget_usd.unwrap_or(0.0),
                    weekly_budget_usd,
                    weekly_spending_usd: 0.0,
                    weekly_usage_percentage: 0.0,
                    weekly_warning_level: "ok".to_string(),
                    weekly_remaining_usd: weekly_budget_usd.unwrap_or(0.0),
                };
                println!("{}", serde_json::to_string_pretty(&export)?);
            },
            OutputFormat::Csv => {
                println!(
                    "period,budget_usd,spending_usd,usage_percentage,warning_level,remaining_usd"
                );
                if let Some(daily) = daily_budget_usd {
                    println!("daily,{},0.0,0.0,ok,{}", daily, daily);
                }
                if let Some(weekly) = weekly_budget_usd {
                    println!("weekly,{},0.0,0.0,ok,{}", weekly, weekly);
                }
            },
            OutputFormat::Text => {
                println!("💰 Budget Status\n");
                println!("⚠️  No analytics data available yet.");
                println!(
                    "   Budget tracking will begin after running specs with the orchestrator."
                );
            },
        }
        return Ok(());
    }

    let store = Store::open(&store_path)?;

    // Create budget tracker with configured threshold
    let tracker = BudgetTracker::new(warn_threshold);

    // Check daily budget
    let daily_status = if let Some(daily) = daily_budget_usd {
        Some(tracker.check_daily_budget(&store, daily)?)
    } else {
        None
    };

    // Check weekly budget
    let weekly_status = if let Some(weekly) = weekly_budget_usd {
        Some(tracker.check_weekly_budget(&store, weekly)?)
    } else {
        None
    };

    // Output based on format
    match format {
        OutputFormat::Text => {
            println!("💰 Budget Status\n");

            if let Some(ref status) = daily_status {
                println!("Daily Budget:");
                print!("  ");
                match status.warning_level {
                    surge_persistence::budget::BudgetWarningLevel::Ok => {
                        println!(
                            "✅ ${:.2}/${:.2} used ({:.1}%)",
                            status.actual_spending_usd,
                            status.budget_limit_usd,
                            status.usage_percentage
                        );
                    },
                    surge_persistence::budget::BudgetWarningLevel::Warning => {
                        println!(
                            "⚠️  Warning: ${:.2}/${:.2} daily budget used ({:.1}%)",
                            status.actual_spending_usd,
                            status.budget_limit_usd,
                            status.usage_percentage
                        );
                    },
                    surge_persistence::budget::BudgetWarningLevel::Critical => {
                        println!(
                            "🚨 Critical: ${:.2}/${:.2} daily budget used ({:.1}%)",
                            status.actual_spending_usd,
                            status.budget_limit_usd,
                            status.usage_percentage
                        );
                    },
                }
                println!("  Remaining: ${:.2}", status.remaining_usd());
                println!();
            }

            if let Some(ref status) = weekly_status {
                println!("Weekly Budget:");
                print!("  ");
                match status.warning_level {
                    surge_persistence::budget::BudgetWarningLevel::Ok => {
                        println!(
                            "✅ ${:.2}/${:.2} used ({:.1}%)",
                            status.actual_spending_usd,
                            status.budget_limit_usd,
                            status.usage_percentage
                        );
                    },
                    surge_persistence::budget::BudgetWarningLevel::Warning => {
                        println!(
                            "⚠️  Warning: ${:.2}/${:.2} weekly budget used ({:.1}%)",
                            status.actual_spending_usd,
                            status.budget_limit_usd,
                            status.usage_percentage
                        );
                    },
                    surge_persistence::budget::BudgetWarningLevel::Critical => {
                        println!(
                            "🚨 Critical: ${:.2}/${:.2} weekly budget used ({:.1}%)",
                            status.actual_spending_usd,
                            status.budget_limit_usd,
                            status.usage_percentage
                        );
                    },
                }
                println!("  Remaining: ${:.2}", status.remaining_usd());
            }
        },
        OutputFormat::Json => {
            let export = BudgetStatusExport {
                daily_budget_usd,
                daily_spending_usd: daily_status
                    .as_ref()
                    .map(|s| s.actual_spending_usd)
                    .unwrap_or(0.0),
                daily_usage_percentage: daily_status
                    .as_ref()
                    .map(|s| s.usage_percentage)
                    .unwrap_or(0.0),
                daily_warning_level: daily_status
                    .as_ref()
                    .map(|s| format!("{:?}", s.warning_level).to_lowercase())
                    .unwrap_or_else(|| "none".to_string()),
                daily_remaining_usd: daily_status
                    .as_ref()
                    .map(|s| s.remaining_usd())
                    .unwrap_or(0.0),
                weekly_budget_usd,
                weekly_spending_usd: weekly_status
                    .as_ref()
                    .map(|s| s.actual_spending_usd)
                    .unwrap_or(0.0),
                weekly_usage_percentage: weekly_status
                    .as_ref()
                    .map(|s| s.usage_percentage)
                    .unwrap_or(0.0),
                weekly_warning_level: weekly_status
                    .as_ref()
                    .map(|s| format!("{:?}", s.warning_level).to_lowercase())
                    .unwrap_or_else(|| "none".to_string()),
                weekly_remaining_usd: weekly_status
                    .as_ref()
                    .map(|s| s.remaining_usd())
                    .unwrap_or(0.0),
            };
            println!("{}", serde_json::to_string_pretty(&export)?);
        },
        OutputFormat::Csv => {
            println!("period,budget_usd,spending_usd,usage_percentage,warning_level,remaining_usd");
            if let Some(ref status) = daily_status {
                println!(
                    "daily,{:.2},{:.2},{:.2},{:?},{:.2}",
                    status.budget_limit_usd,
                    status.actual_spending_usd,
                    status.usage_percentage,
                    format!("{:?}", status.warning_level).to_lowercase(),
                    status.remaining_usd()
                );
            }
            if let Some(ref status) = weekly_status {
                println!(
                    "weekly,{:.2},{:.2},{:.2},{:?},{:.2}",
                    status.budget_limit_usd,
                    status.actual_spending_usd,
                    status.usage_percentage,
                    format!("{:?}", status.warning_level).to_lowercase(),
                    status.remaining_usd()
                );
            }
        },
    }

    Ok(())
}
