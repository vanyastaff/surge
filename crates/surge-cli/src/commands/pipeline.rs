use std::io::Write as _;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use surge_core::{SurgeConfig, SurgeEvent};
use surge_core::state::TaskState;

use super::load_spec_by_id;

/// Accumulated token and cost totals for a pipeline run.
#[derive(Debug, Clone, Default)]
struct RunTotals {
    input_tokens: u64,
    output_tokens: u64,
    thought_tokens: u64,
    total_cost: f64,
}

/// QA summary information for display.
#[derive(Debug, Clone, Default)]
struct QaSummary {
    verdict: Option<String>,
    reasoning: Option<String>,
    iterations: u32,
}

/// Run a spec through the full pipeline.
///
/// Exit codes:
///   0 — completed
///   3 — paused
///   4 — failed
pub async fn run(
    spec_id: String,
    parallel: Option<usize>,
    _planner: Option<String>,
    _coder: Option<String>,
    resume: bool,
) -> Result<()> {
    let mut config = SurgeConfig::load_or_default()?;
    config.apply_env_overrides();

    if let Some(p) = parallel {
        config.pipeline.max_parallel = p;
    }

    let mut spec_file = load_spec_by_id(&spec_id)?;

    // Handle resume logic if requested
    if resume
        && let Ok(store_path) = surge_persistence::store::Store::default_path()
        && store_path.exists()
        && let Ok(store) = surge_persistence::store::Store::open(&store_path)
        && let Ok(checkpoints) = store.list_task_states_by_spec(spec_file.spec.id)
        && let Some((_, state, _)) = checkpoints.first()
        && let surge_core::state::TaskState::Executing { completed, total } = state
    {
        if *completed > 0 {
            println!("📍 Resuming from checkpoint: {completed}/{total} subtasks completed");

            // Build dependency graph to get subtasks in execution order
            if let Ok(graph) = surge_spec::DependencyGraph::from_spec(&spec_file.spec)
                && let Ok(batch_ids) = graph.topological_batches()
            {
                // Flatten batch IDs into execution order
                let execution_order: Vec<_> = batch_ids.iter().flatten().copied().collect();

                // Mark the first N subtasks as completed
                let mut marked = 0;
                for subtask_id in execution_order.iter().take(*completed) {
                    if let Some(subtask) = spec_file.spec.subtasks.iter_mut().find(|s| s.id == *subtask_id) {
                        subtask.execution.state = surge_core::spec::SubtaskState::Completed;
                        marked += 1;
                    }
                }

                println!("   ✓ Skipping {marked} completed subtask{}", if marked == 1 { "" } else { "s" });
            }
        } else {
            println!("ℹ️  No completed subtasks to resume from");
        }
    }

    println!("⚡ Running spec: {}", spec_file.spec.title);
    println!("   Subtasks: {}", spec_file.spec.subtasks.len());

    let cwd = std::env::current_dir()?;
    let orch_config = surge_orchestrator::OrchestratorConfig {
        surge_config: config,
        working_dir: cwd,
    };
    let orchestrator = surge_orchestrator::Orchestrator::new(orch_config);

    let totals = Arc::new(Mutex::new(RunTotals::default()));
    let totals_clone = Arc::clone(&totals);

    let qa_summary = Arc::new(Mutex::new(QaSummary::default()));
    let qa_summary_clone = Arc::clone(&qa_summary);

    let mut events = orchestrator.subscribe();
    tokio::spawn(async move {

        while let Ok(event) = events.recv().await {
            match event {
                SurgeEvent::SubtaskStarted { subtask_id, .. } => {
                    // Clear the token counter line before printing subtask status
                    print!("\r\x1b[K");
                    let _ = std::io::stdout().flush();
                    println!("  ▶ Starting subtask {subtask_id}");
                }
                SurgeEvent::SubtaskCompleted { subtask_id, success, .. } => {
                    // Clear the token counter line before printing subtask status
                    print!("\r\x1b[K");
                    let _ = std::io::stdout().flush();
                    let mark = if success { "✅" } else { "❌" };
                    println!("  {mark} Subtask {subtask_id}");
                }
                SurgeEvent::TaskStateChanged { new_state, .. } => {
                    // Capture QA state information
                    match &new_state {
                        TaskState::QaReview { verdict, reasoning } => {
                            let mut qa = qa_summary_clone.lock().unwrap();
                            qa.iterations = qa.iterations.max(1);
                            if let Some(v) = verdict {
                                qa.verdict = Some(v.clone());
                            }
                            if let Some(r) = reasoning {
                                qa.reasoning = Some(r.clone());
                            }
                        }
                        TaskState::QaFix { iteration, verdict, reasoning } => {
                            let mut qa = qa_summary_clone.lock().unwrap();
                            qa.iterations = *iteration;
                            if let Some(v) = verdict {
                                qa.verdict = Some(v.clone());
                            }
                            if let Some(r) = reasoning {
                                qa.reasoning = Some(r.clone());
                            }
                        }
                        _ => {}
                    }

                    // Clear the token counter line before printing state change
                    print!("\r\x1b[K");
                    let _ = std::io::stdout().flush();
                    println!("  📊 State: {new_state}");
                }
                SurgeEvent::TokensConsumed {
                    input_tokens: input,
                    output_tokens: output,
                    thought_tokens: thought,
                    estimated_cost_usd,
                    ..
                } => {
                    // Update cumulative totals
                    let mut t = totals_clone.lock().unwrap();
                    t.input_tokens += input;
                    t.output_tokens += output;
                    if let Some(th) = thought {
                        t.thought_tokens += th;
                    }
                    if let Some(cost) = estimated_cost_usd {
                        t.total_cost += cost;
                    }

                    // Calculate total tokens
                    let total = t.input_tokens + t.output_tokens + t.thought_tokens;

                    // Display live counter on the same line
                    print!(
                        "\r💰 Tokens: {} in / {} out / {} total | Cost: ${:.4}",
                        format_tokens(t.input_tokens),
                        format_tokens(t.output_tokens),
                        format_tokens(total),
                        t.total_cost
                    );
                    let _ = std::io::stdout().flush();
                }
                _ => {}
            }
        }
    });

    let result = orchestrator.execute(&mut spec_file).await;

    // Clear the token counter line before printing final result
    print!("\r\x1b[K");
    let _ = std::io::stdout().flush();

    match result {
        surge_orchestrator::PipelineResult::Completed => {
            println!("✅ Pipeline completed successfully!");
        }
        surge_orchestrator::PipelineResult::Paused { phase, reason } => {
            println!("⏸️  Pipeline paused at {phase}: {reason}");
            std::process::exit(3);
        }
        surge_orchestrator::PipelineResult::Failed { reason } => {
            println!("❌ Pipeline failed: {reason}");
            std::process::exit(4);
        }
    }

    // Display final cost summary
    let final_totals = totals.lock().unwrap();
    if final_totals.input_tokens > 0 || final_totals.output_tokens > 0 {
        println!();
        println!("💰 Token Usage Summary:");
        println!("   Input tokens:   {}", format_tokens(final_totals.input_tokens));
        println!("   Output tokens:  {}", format_tokens(final_totals.output_tokens));
        if final_totals.thought_tokens > 0 {
            println!("   Thought tokens: {}", format_tokens(final_totals.thought_tokens));
        }
        let total_tokens = final_totals.input_tokens + final_totals.output_tokens + final_totals.thought_tokens;
        println!("   Total tokens:   {}", format_tokens(total_tokens));
        println!("   Estimated cost: ${:.4}", final_totals.total_cost);
    }

    // Display QA summary if QA was performed
    let final_qa = qa_summary.lock().unwrap();
    if final_qa.iterations > 0 {
        println!();
        println!("🔍 QA Review Summary:");
        if let Some(verdict) = &final_qa.verdict {
            println!("   Verdict:    {}", verdict);
        }
        if let Some(reasoning) = &final_qa.reasoning {
            println!("   Reasoning:  {}", reasoning);
        }
        println!("   Iterations: {}", final_qa.iterations);
    }

    Ok(())
}

/// Show pipeline status for a spec.
pub fn status(spec_id: String) -> Result<()> {
    let spec_file = load_spec_by_id(&spec_id)?;
    let spec = &spec_file.spec;

    println!("⚡ {}", spec.title);
    println!("   ID:          {}", spec.id);
    println!("   Complexity:  {:?}", spec.complexity);
    println!("   Subtasks:    {}", spec.subtasks.len());

    if !spec.subtasks.is_empty() {
        println!();
        for sub in &spec.subtasks {
            let ac_done = sub.acceptance_criteria.iter().filter(|a| a.met).count();
            let ac_total = sub.acceptance_criteria.len();
            if ac_total > 0 {
                println!("   ⬜ {} ({}/{} criteria met)", sub.title, ac_done, ac_total);
            } else {
                println!("   ⬜ {}", sub.title);
            }
        }
    }

    // Show token and cost data if available
    if let Ok(store_path) = surge_persistence::store::Store::default_path()
        && store_path.exists()
        && let Ok(store) = surge_persistence::store::Store::open(&store_path)
        && let Ok(Some(spec_usage)) = store.get_spec(spec.id)
    {
        println!();
        println!("💰 Token Usage:");
        println!("   Sessions:       {}", spec_usage.session_count);
        println!("   Input tokens:   {}", format_tokens(spec_usage.input_tokens));
        println!("   Output tokens:  {}", format_tokens(spec_usage.output_tokens));
        if spec_usage.thought_tokens > 0 {
            println!("   Thought tokens: {}", format_tokens(spec_usage.thought_tokens));
        }
        if spec_usage.cached_read_tokens > 0 {
            println!("   Cached read:    {}", format_tokens(spec_usage.cached_read_tokens));
        }
        if spec_usage.cached_write_tokens > 0 {
            println!("   Cached write:   {}", format_tokens(spec_usage.cached_write_tokens));
        }
        let total_tokens = spec_usage.input_tokens + spec_usage.output_tokens + spec_usage.thought_tokens;
        println!("   Total tokens:   {}", format_tokens(total_tokens));
        println!("   Estimated cost: ${:.4}", spec_usage.estimated_cost_usd);
    }

    // Show worktree info if available
    if let Ok(git) = surge_git::GitManager::discover()
        && let Ok(worktrees) = git.list_worktrees()
    {
        let spec_id_str = spec.id.to_string();
        if let Some(wt) = worktrees.iter().find(|w| w.spec_id.contains(&spec_id_str)) {
            println!();
            println!(
                "   Worktree: {} {}",
                if wt.exists_on_disk { "✅" } else { "❌ (missing)" },
                wt.path.display()
            );
            println!("   Branch:   {}", wt.branch);
        }
    }

    Ok(())
}

/// Show or follow pipeline logs for a spec.
pub fn logs(spec_id: String, follow: bool) -> Result<()> {
    let specs_dir = surge_spec::SpecFile::specs_dir()?;

    // Accept either full spec ID or prefix
    let spec_id_full = resolve_spec_id_for_logs(&spec_id, &specs_dir)?;
    let log_path = specs_dir.join(&spec_id_full).join("pipeline.log");

    if !log_path.exists() {
        anyhow::bail!(
            "No logs found for '{}'. Run 'surge run {}' first.",
            spec_id,
            spec_id
        );
    }

    if follow {
        // Read existing content then watch for new lines
        let existing = std::fs::read_to_string(&log_path)?;
        print!("{existing}");

        let mut pos = existing.len() as u64;
        println!("--- following (Ctrl-C to stop) ---");
        loop {
            std::thread::sleep(std::time::Duration::from_millis(200));
            let file = std::fs::File::open(&log_path)?;
            let metadata = file.metadata()?;
            let new_len = metadata.len();
            if new_len > pos {
                use std::io::{Read, Seek, SeekFrom};
                let mut f = file;
                f.seek(SeekFrom::Start(pos))?;
                let mut buf = String::new();
                f.read_to_string(&mut buf)?;
                print!("{buf}");
                pos = new_len;
            }
        }
    } else {
        print!("{}", std::fs::read_to_string(&log_path)?);
    }

    Ok(())
}

/// Plan a spec — show execution order without running it.
pub fn plan(spec_id: String, _agent: Option<String>) -> Result<()> {
    let spec_file = load_spec_by_id(&spec_id)?;
    let spec = &spec_file.spec;

    // Validate first
    let validation = surge_spec::validate_spec(spec);
    if !validation.is_ok() {
        println!("❌ Spec validation failed:");
        for e in &validation.errors {
            println!("   ❌ {e}");
        }
        std::process::exit(1);
    }

    println!("⚡ Plan: {}\n", spec.title);
    println!("   ID:          {}", spec.id);
    println!("   Complexity:  {:?}", spec.complexity);
    println!("   Description: {}", spec.description);

    if spec.subtasks.is_empty() {
        println!("\n   (no subtasks — run 'surge spec show {}' to inspect)", spec_id);
        return Ok(());
    }

    let graph = surge_spec::DependencyGraph::from_spec(spec)?;
    let batches = graph.topological_batches()?;

    println!("\n   Execution plan ({} subtasks, {} batch{}):\n",
        spec.subtasks.len(),
        batches.len(),
        if batches.len() == 1 { "" } else { "es" }
    );

    for (i, batch) in batches.iter().enumerate() {
        let parallel_note = if batch.len() > 1 {
            format!(" (parallel × {})", batch.len())
        } else {
            String::new()
        };
        println!("   Batch {}{}", i + 1, parallel_note);

        for id in batch {
            if let Some(sub) = spec.subtasks.iter().find(|s| s.id == *id) {
                println!("     ⬜ {} — {:?}", sub.title, sub.complexity);
                if !sub.files.is_empty() {
                    println!("        Files: {}", sub.files.join(", "));
                }
            }
        }
        println!();
    }

    if !validation.warnings.is_empty() {
        for w in &validation.warnings {
            println!("   ⚠️  {w}");
        }
        println!();
    }

    println!("   Run 'surge run {}' to execute.", spec_id);

    Ok(())
}

/// Find a spec directory by ID prefix for log lookup.
fn resolve_spec_id_for_logs(id: &str, specs_dir: &std::path::Path) -> Result<String> {
    // Check exact match first
    if specs_dir.join(id).is_dir() {
        return Ok(id.to_string());
    }
    // Try prefix match among subdirectories
    if let Ok(entries) = std::fs::read_dir(specs_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with(id) && entry.path().is_dir() {
                return Ok(name);
            }
        }
    }
    Ok(id.to_string())
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
