use std::io::{self, BufRead, Write as _};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use surge_core::config::GateDecision;
use surge_core::state::TaskState;
use surge_core::{SurgeConfig, SurgeEvent};

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
                    if let Some(subtask) = spec_file
                        .spec
                        .subtasks
                        .iter_mut()
                        .find(|s| s.id == *subtask_id)
                    {
                        subtask.execution.state = surge_core::spec::SubtaskState::Completed;
                        marked += 1;
                    }
                }

                println!(
                    "   ✓ Skipping {marked} completed subtask{}",
                    if marked == 1 { "" } else { "s" }
                );
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
                },
                SurgeEvent::SubtaskCompleted {
                    subtask_id,
                    success,
                    ..
                } => {
                    // Clear the token counter line before printing subtask status
                    print!("\r\x1b[K");
                    let _ = std::io::stdout().flush();
                    let mark = if success { "✅" } else { "❌" };
                    println!("  {mark} Subtask {subtask_id}");
                },
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
                        },
                        TaskState::QaFix {
                            iteration,
                            verdict,
                            reasoning,
                        } => {
                            let mut qa = qa_summary_clone.lock().unwrap();
                            qa.iterations = *iteration;
                            if let Some(v) = verdict {
                                qa.verdict = Some(v.clone());
                            }
                            if let Some(r) = reasoning {
                                qa.reasoning = Some(r.clone());
                            }
                        },
                        _ => {},
                    }

                    // Clear the token counter line before printing state change
                    print!("\r\x1b[K");
                    let _ = std::io::stdout().flush();
                    println!("  📊 State: {new_state}");
                },
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
                        super::format::format_number(t.input_tokens),
                        super::format::format_number(t.output_tokens),
                        super::format::format_number(total),
                        t.total_cost
                    );
                    let _ = std::io::stdout().flush();
                },
                SurgeEvent::GateAwaitingApproval {
                    gate_name, reason, ..
                } => {
                    // Clear the token counter line before showing gate prompt
                    print!("\r\x1b[K");
                    let _ = std::io::stdout().flush();

                    // Note: The actual gate approval handling will be done synchronously
                    // in the orchestrator. This event is just for display purposes.
                    println!("  🚦 Gate awaiting approval: {}", gate_name);
                    if let Some(r) = &reason {
                        println!("     Reason: {}", r);
                    }

                    // Show plan preview if available
                    if let Some(preview) = format_plan_preview(&gate_name) {
                        print!("  {}", preview);
                    }

                    // Show diff summary if available
                    if let Some(diff) = format_diff_summary() {
                        print!("  {}", diff);
                    }
                },
                SurgeEvent::GateApproved {
                    gate_name,
                    approved_by,
                    ..
                } => {
                    // Clear the token counter line before showing gate result
                    print!("\r\x1b[K");
                    let _ = std::io::stdout().flush();

                    print!("  ✅ Gate approved: {}", gate_name);
                    if let Some(by) = approved_by {
                        print!(" (by {})", by);
                    }
                    println!();
                },
                SurgeEvent::GateRejected {
                    gate_name,
                    rejected_by,
                    reason,
                    ..
                } => {
                    // Clear the token counter line before showing gate result
                    print!("\r\x1b[K");
                    let _ = std::io::stdout().flush();

                    print!("  ❌ Gate rejected: {}", gate_name);
                    if let Some(by) = rejected_by {
                        print!(" (by {})", by);
                    }
                    if let Some(r) = reason {
                        print!(" - {}", r);
                    }
                    println!();
                },
                _ => {},
            }
        }
    });

    // Run the pipeline, handling gate pauses with interactive prompts
    let result = loop {
        let result = orchestrator.execute(&mut spec_file).await;

        match result {
            surge_orchestrator::PipelineResult::Paused { phase, reason } => {
                // Clear the token counter line before showing gate prompt
                print!("\r\x1b[K");
                let _ = std::io::stdout().flush();

                // Determine gate name from reason
                let gate_name = if reason.contains("after_spec") {
                    "after_spec"
                } else if reason.contains("after_plan") {
                    "after_plan"
                } else if reason.contains("after_each_subtask") {
                    "after_each_subtask"
                } else if reason.contains("after_qa") {
                    "after_qa"
                } else {
                    "unknown_gate"
                };

                // Get user decision via interactive prompt
                match prompt_gate_approval(gate_name, Some(&reason), spec_file.spec.id, phase) {
                    Ok(decision) => {
                        // Persist the decision to DECISION.json
                        let specs_dir = surge_spec::SpecFile::specs_dir()?;
                        let gate_config = surge_core::config::GateConfig::default();
                        let gate_manager =
                            surge_orchestrator::gates::GateManager::new(gate_config, specs_dir);
                        gate_manager.record_decision(spec_file.spec.id, phase, decision.clone());

                        // Check if user aborted
                        if matches!(decision, GateDecision::Aborted { .. }) {
                            println!("🛑 Pipeline aborted by user");
                            std::process::exit(4);
                        }

                        // Continue to next iteration to resume pipeline
                        println!("▶️  Resuming pipeline...\n");
                    },
                    Err(e) => {
                        println!("❌ Failed to get gate approval: {e}");
                        std::process::exit(4);
                    },
                }
            },
            other => break other,
        }
    };

    // Clear the token counter line before printing final result
    print!("\r\x1b[K");
    let _ = std::io::stdout().flush();

    match result {
        surge_orchestrator::PipelineResult::Completed => {
            println!("✅ Pipeline completed successfully!");
        },
        surge_orchestrator::PipelineResult::Paused { phase, reason } => {
            // This shouldn't happen anymore since we handle pauses in the loop
            println!("⏸️  Pipeline paused at {phase}: {reason}");
            std::process::exit(3);
        },
        surge_orchestrator::PipelineResult::Failed { reason } => {
            println!("❌ Pipeline failed: {reason}");
            std::process::exit(4);
        },
    }

    // Display final cost summary
    let final_totals = totals.lock().unwrap();
    if final_totals.input_tokens > 0 || final_totals.output_tokens > 0 {
        println!();
        println!("💰 Token Usage Summary:");
        println!(
            "   Input tokens:   {}",
            super::format::format_number(final_totals.input_tokens)
        );
        println!(
            "   Output tokens:  {}",
            super::format::format_number(final_totals.output_tokens)
        );
        if final_totals.thought_tokens > 0 {
            println!(
                "   Thought tokens: {}",
                super::format::format_number(final_totals.thought_tokens)
            );
        }
        let total_tokens =
            final_totals.input_tokens + final_totals.output_tokens + final_totals.thought_tokens;
        println!(
            "   Total tokens:   {}",
            super::format::format_number(total_tokens)
        );
        println!("   Estimated cost: ${:.4}", final_totals.total_cost);
    }

    // Display QA summary if QA was performed
    let final_qa = qa_summary.lock().unwrap();
    if final_qa.iterations > 0 {
        println!();
        println!("🔍 QA Review Summary:");

        // Use enhanced verdict formatter if available
        if let Some(formatted_verdict) =
            format_qa_verdict(final_qa.verdict.as_deref(), final_qa.reasoning.as_deref())
        {
            print!("{}", formatted_verdict);
        } else {
            if let Some(verdict) = &final_qa.verdict {
                println!("   Verdict:    {}", verdict);
            }
            if let Some(reasoning) = &final_qa.reasoning {
                println!("   Reasoning:  {}", reasoning);
            }
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
                println!(
                    "   ⬜ {} ({}/{} criteria met)",
                    sub.title, ac_done, ac_total
                );
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
        println!(
            "   Input tokens:   {}",
            super::format::format_number(spec_usage.input_tokens)
        );
        println!(
            "   Output tokens:  {}",
            super::format::format_number(spec_usage.output_tokens)
        );
        if spec_usage.thought_tokens > 0 {
            println!(
                "   Thought tokens: {}",
                super::format::format_number(spec_usage.thought_tokens)
            );
        }
        if spec_usage.cached_read_tokens > 0 {
            println!(
                "   Cached read:    {}",
                super::format::format_number(spec_usage.cached_read_tokens)
            );
        }
        if spec_usage.cached_write_tokens > 0 {
            println!(
                "   Cached write:   {}",
                super::format::format_number(spec_usage.cached_write_tokens)
            );
        }
        let total_tokens =
            spec_usage.input_tokens + spec_usage.output_tokens + spec_usage.thought_tokens;
        println!(
            "   Total tokens:   {}",
            super::format::format_number(total_tokens)
        );
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
                if wt.exists_on_disk {
                    "✅"
                } else {
                    "❌ (missing)"
                },
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
        println!(
            "\n   (no subtasks — run 'surge spec show {}' to inspect)",
            spec_id
        );
        return Ok(());
    }

    let graph = surge_spec::DependencyGraph::from_spec(spec)?;
    let batches = graph.topological_batches()?;

    println!(
        "\n   Execution plan ({} subtasks, {} batch{}):\n",
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

/// Skip a subtask by marking it as skipped.
pub fn skip(spec_id: String, subtask_id: String) -> Result<()> {
    let mut spec_file = load_spec_by_id(&spec_id)?;

    // Find the subtask
    let subtask = spec_file
        .spec
        .subtasks
        .iter_mut()
        .find(|s| s.id.to_string() == subtask_id)
        .ok_or_else(|| anyhow::anyhow!("Subtask '{}' not found in spec", subtask_id))?;

    // Check if already skipped
    if subtask.execution.state == surge_core::spec::SubtaskState::Skipped {
        println!("⚠️  Subtask '{}' is already skipped", subtask_id);
        return Ok(());
    }

    // Check if already completed
    if subtask.execution.state == surge_core::spec::SubtaskState::Completed {
        println!("⚠️  Subtask '{}' is already completed", subtask_id);
        return Ok(());
    }

    // Mark as skipped
    subtask.execution.state = surge_core::spec::SubtaskState::Skipped;

    // Save the spec
    spec_file.save_in_place()?;

    println!("✅ Subtask '{}' marked as skipped", subtask_id);

    Ok(())
}

/// Pause a running task.
pub fn pause(task_id: String) -> Result<()> {
    let spec_file = load_spec_by_id(&task_id)?;
    let spec = &spec_file.spec;

    println!("⏸️  Pausing task: {}", spec.title);
    println!("   ID: {}", spec.id);

    // Check if there's a checkpoint in the persistence store
    if let Ok(store_path) = surge_persistence::store::Store::default_path()
        && store_path.exists()
        && let Ok(store) = surge_persistence::store::Store::open(&store_path)
        && let Ok(checkpoints) = store.list_task_states_by_spec(spec.id)
        && let Some((_, state, _)) = checkpoints.first()
    {
        println!("   Current state: {}", state);
        println!();
        println!(
            "ℹ️  Task execution can be resumed with: surge resume {}",
            task_id
        );
    } else {
        println!();
        println!("ℹ️  No active execution found for this task");
    }

    Ok(())
}

/// Resume a paused task.
pub async fn resume(task_id: String) -> Result<()> {
    println!("🔄 Resuming task: {}", task_id);
    println!();

    // Resume by calling run with resume flag set
    run(task_id, None, None, None, true).await
}

/// Format a plan preview showing what comes next in the pipeline.
///
/// Returns a formatted string showing the next phase or stage information,
/// or None if no preview is available.
fn format_plan_preview(gate_name: &str) -> Option<String> {
    // Map gate names to next phase descriptions
    let next_phase = match gate_name {
        "post_planning" => "Execution phase — agents will implement subtasks in parallel",
        "post_execution" => "QA Review — automated quality checks and validation",
        "post_qa" => "Human Review — final approval before merging",
        "pre_merge" => "Merge phase — changes will be integrated into main branch",
        _ => return None,
    };

    Some(format!("   📋 Next: {}\n", next_phase))
}

/// Format a diff summary showing git changes statistics.
///
/// Returns a formatted string with files changed, insertions, and deletions,
/// or None if git operations fail or there are no changes.
fn format_diff_summary() -> Option<String> {
    use std::process::Command;

    let output = match Command::new("git")
        .args(["diff", "--stat", "HEAD"])
        .output()
    {
        Ok(o) => o,
        Err(e) => {
            tracing::debug!("git diff --stat failed to execute: {e}");
            return None;
        },
    };

    if !output.status.success() {
        tracing::debug!(
            "git diff --stat exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        return None;
    }

    if output.stdout.is_empty() {
        return None;
    }

    let diff_stat = String::from_utf8_lossy(&output.stdout);
    let summary_line = diff_stat.lines().last()?;

    if summary_line.trim().is_empty() {
        return None;
    }

    Some(format!("   📊 Changes: {}\n", summary_line.trim()))
}

/// Format QA verdict display with enhanced formatting.
///
/// Returns a formatted string showing QA verdict and reasoning,
/// or None if no QA data is available.
fn format_qa_verdict(verdict: Option<&str>, reasoning: Option<&str>) -> Option<String> {
    let v = verdict?;

    let verdict_icon = match v.to_lowercase().as_str() {
        "pass" | "approved" | "success" => "✅",
        "fail" | "rejected" | "failure" => "❌",
        "warning" | "needs_work" => "⚠️",
        _ => "🔍",
    };

    let mut output = format!("   {} QA Verdict: {}\n", verdict_icon, v);

    if let Some(r) = reasoning {
        // Simple word wrapping for reasoning text
        let max_width = 70;
        if r.len() <= max_width {
            output.push_str(&format!("   Reasoning: {}\n", r));
        } else {
            output.push_str("   Reasoning:\n");
            let words: Vec<&str> = r.split_whitespace().collect();
            let mut current_line = String::new();

            for word in words {
                if current_line.len() + word.len() + 1 > max_width && !current_line.is_empty() {
                    output.push_str(&format!("      {}\n", current_line));
                    current_line.clear();
                }
                if !current_line.is_empty() {
                    current_line.push(' ');
                }
                current_line.push_str(word);
            }

            if !current_line.is_empty() {
                output.push_str(&format!("      {}\n", current_line));
            }
        }
    }

    Some(output)
}

/// Prompt the user for gate approval decision.
///
/// Displays an interactive prompt asking the user to approve, reject, or abort
/// the pipeline at a gate. Returns the user's decision or an error if input fails.
/// The decision is NOT persisted by this function - the caller is responsible for
/// persisting it via `GateManager::record_decision`.
///
/// # Arguments
///
/// * `gate_name` - Name of the gate requiring approval
/// * `reason` - Optional reason why approval is needed
/// * `spec_id` - The spec ID for context (used for logging/display)
/// * `phase` - The pipeline phase for context (used for logging/display)
///
/// # Returns
///
/// A `GateDecision` representing the user's choice:
/// - `Approved` - Continue to next phase (optionally with feedback)
/// - `Rejected` - Re-run phase with structured feedback
/// - `Aborted` - Terminate the task
pub fn prompt_gate_approval(
    gate_name: &str,
    reason: Option<&str>,
    _spec_id: surge_core::id::SpecId,
    _phase: surge_orchestrator::phases::Phase,
) -> Result<GateDecision> {
    // Clear the token counter line before displaying the prompt
    print!("\r\x1b[K");
    let _ = io::stdout().flush();

    println!();
    println!("🚦 Gate: {}", gate_name);
    if let Some(r) = reason {
        println!("   Reason: {}", r);
    }
    println!();

    // Display plan preview if available
    if let Some(preview) = format_plan_preview(gate_name) {
        print!("{}", preview);
    }

    // Display diff summary if available
    if let Some(diff) = format_diff_summary() {
        print!("{}", diff);
    }

    // Display QA verdict if this is a post-QA gate
    if gate_name.contains("qa") || gate_name.contains("post_qa") {
        // Try to read QA verdict from current context
        // For now, we'll skip this since we don't have access to the context here
        // This could be enhanced by passing QA state to the prompt function
    }

    println!();
    println!("   Choose an action:");
    println!("   [a] Approve — continue to next phase");
    println!("   [r] Reject — re-run phase with feedback");
    println!("   [x] Abort — terminate task");
    println!();
    print!("   Your choice (a/r/x): ");
    io::stdout().flush()?;

    let stdin = io::stdin();
    let mut lines = stdin.lock().lines();

    loop {
        if let Some(line) = lines.next() {
            let input = line?.trim().to_lowercase();

            match input.as_str() {
                "a" | "approve" => {
                    print!("   Optional feedback (press Enter to skip): ");
                    io::stdout().flush()?;

                    let feedback = if let Some(line) = lines.next() {
                        let text = line?.trim().to_string();
                        if text.is_empty() { None } else { Some(text) }
                    } else {
                        None
                    };

                    return Ok(GateDecision::Approved { feedback });
                },
                "r" | "reject" => {
                    print!("   Rejection reason: ");
                    io::stdout().flush()?;

                    let reason = if let Some(line) = lines.next() {
                        line?.trim().to_string()
                    } else {
                        "No reason provided".to_string()
                    };

                    print!("   Feedback for next iteration: ");
                    io::stdout().flush()?;

                    let feedback = if let Some(line) = lines.next() {
                        line?.trim().to_string()
                    } else {
                        "Please address the issues".to_string()
                    };

                    return Ok(GateDecision::Rejected { reason, feedback });
                },
                "x" | "abort" => {
                    print!("   Abort reason: ");
                    io::stdout().flush()?;

                    let reason = if let Some(line) = lines.next() {
                        line?.trim().to_string()
                    } else {
                        "Task aborted by user".to_string()
                    };

                    return Ok(GateDecision::Aborted { reason });
                },
                _ => {
                    println!("   Invalid choice. Please enter 'a', 'r', or 'x'.");
                    print!("   Your choice (a/r/x): ");
                    io::stdout().flush()?;
                },
            }
        } else {
            return Err(anyhow::anyhow!("Failed to read user input"));
        }
    }
}
