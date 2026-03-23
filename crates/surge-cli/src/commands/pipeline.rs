use anyhow::Result;
use surge_core::{SurgeConfig, SurgeEvent};

use super::load_spec_by_id;

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
) -> Result<()> {
    let mut config = SurgeConfig::load_or_default()?;
    config.apply_env_overrides();

    if let Some(p) = parallel {
        config.pipeline.max_parallel = p;
    }

    let spec_file = load_spec_by_id(&spec_id)?;

    println!("⚡ Running spec: {}", spec_file.spec.title);
    println!("   Subtasks: {}", spec_file.spec.subtasks.len());

    let cwd = std::env::current_dir()?;
    let orch_config = surge_orchestrator::OrchestratorConfig {
        surge_config: config,
        working_dir: cwd,
    };
    let orchestrator = surge_orchestrator::Orchestrator::new(orch_config);

    let mut events = orchestrator.subscribe();
    tokio::spawn(async move {
        while let Ok(event) = events.recv().await {
            match event {
                SurgeEvent::SubtaskStarted { subtask_id, .. } => {
                    println!("  ▶ Starting subtask {subtask_id}");
                }
                SurgeEvent::SubtaskCompleted { subtask_id, success, .. } => {
                    let mark = if success { "✅" } else { "❌" };
                    println!("  {mark} Subtask {subtask_id}");
                }
                SurgeEvent::TaskStateChanged { new_state, .. } => {
                    println!("  📊 State: {new_state}");
                }
                _ => {}
            }
        }
    });

    let result = orchestrator.execute(&spec_file).await;

    match result {
        surge_orchestrator::PipelineResult::Completed => {
            println!("\n✅ Pipeline completed successfully!");
        }
        surge_orchestrator::PipelineResult::Paused { phase, reason } => {
            println!("\n⏸️  Pipeline paused at {phase}: {reason}");
            std::process::exit(3);
        }
        surge_orchestrator::PipelineResult::Failed { reason } => {
            println!("\n❌ Pipeline failed: {reason}");
            std::process::exit(4);
        }
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
