//! `surge bootstrap` — adaptive bootstrap flow entrypoint.

use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use clap::{Args, Subcommand};
use surge_core::id::RunId;
use surge_core::run_event::{BootstrapStage, EventPayload};
use surge_git::{GitManager, WorktreeLocation};
use surge_orchestrator::bootstrap_driver::{
    MaterializedRun, materialized_run_from_completed, run_bootstrap_in_worktree,
};
use surge_orchestrator::engine::handle::{EngineRunEvent, RunOutcome};
use surge_orchestrator::engine::{Engine, EngineConfig, EngineRunConfig};
use surge_persistence::runs::{EventSeq, Storage};

/// Arguments for `surge bootstrap`.
#[derive(Args, Debug)]
pub struct BootstrapArgs {
    /// Free-form task prompt used by the Description Author.
    pub prompt: Option<String>,
    /// Managed worktree parent directory. Default: sibling `.surge-worktrees/`.
    #[arg(long = "worktree-root", alias = "worktree")]
    pub worktree_root: Option<PathBuf>,
    /// Resume an existing bootstrap run.
    #[command(subcommand)]
    pub command: Option<BootstrapCommands>,
}

/// Subcommands under `surge bootstrap`.
#[derive(Subcommand, Debug)]
pub enum BootstrapCommands {
    /// Resume a previously interrupted bootstrap run.
    Resume {
        /// Bootstrap `RunId` to resume.
        run_id: String,
    },
}

/// Top-level dispatcher for `surge bootstrap`.
pub async fn run(args: BootstrapArgs) -> Result<()> {
    match args.command {
        Some(BootstrapCommands::Resume { run_id }) => {
            resume_command(run_id, args.worktree_root).await
        },
        None => {
            let prompt = args.prompt.ok_or_else(|| {
                anyhow!("provide a prompt or use `surge bootstrap resume <run_id>`")
            })?;
            prompt_command(prompt, args.worktree_root).await
        },
    }
}

async fn prompt_command(prompt: String, worktree_root: Option<PathBuf>) -> Result<()> {
    let bootstrap_run_id = RunId::new();
    println!("bootstrap_run_id={bootstrap_run_id}");
    let worktree = create_bootstrap_worktree(&bootstrap_run_id, worktree_root)?;
    let (engine, storage) = build_local_engine(&worktree).await?;

    let mut approvals = tokio::spawn(poll_console_approvals(
        engine.clone(),
        storage,
        bootstrap_run_id,
    ));
    let driver_engine = engine.clone();
    let driver_worktree = worktree.clone();
    let driver = tokio::spawn(async move {
        run_bootstrap_in_worktree(
            driver_engine.as_ref(),
            prompt,
            bootstrap_run_id,
            driver_worktree,
        )
        .await
    });

    let materialized = tokio::select! {
        result = driver => {
            approvals.abort();
            result.context("bootstrap driver task panicked")??
        }
        approval_result = &mut approvals => {
            approval_result.context("approval task panicked")??;
            return Err(anyhow!("approval loop ended before bootstrap completed"));
        }
    };

    start_followup_run(engine, materialized, worktree).await?;
    Ok(())
}

async fn resume_command(run_id: String, worktree_root: Option<PathBuf>) -> Result<()> {
    let bootstrap_run_id = parse_run_id(&run_id)?;
    let worktree = existing_bootstrap_worktree(&bootstrap_run_id, worktree_root)?;
    let (engine, _storage) = build_local_engine(&worktree).await?;
    let handle = engine
        .resume_run(bootstrap_run_id, worktree.clone())
        .await?;
    let outcome = drive_run_handle(engine.clone(), handle).await?;
    match outcome {
        RunOutcome::Completed { .. } => {},
        RunOutcome::Failed { error } => return Err(anyhow!("bootstrap run failed: {error}")),
        RunOutcome::Aborted { reason } => return Err(anyhow!("bootstrap run aborted: {reason}")),
        _ => return Err(anyhow!("bootstrap run reached an unknown terminal outcome")),
    }

    let materialized = materialized_run_from_completed(engine.as_ref(), bootstrap_run_id).await?;
    start_followup_run(engine, materialized, worktree).await?;
    Ok(())
}

async fn start_followup_run(
    engine: Arc<Engine>,
    materialized: MaterializedRun,
    worktree: PathBuf,
) -> Result<RunOutcome> {
    let followup_run_id = RunId::new();
    println!("followup_run_id={followup_run_id}");
    let handle = engine
        .start_run(
            followup_run_id,
            materialized.materialized_graph,
            worktree,
            EngineRunConfig {
                bootstrap_parent: Some(materialized.bootstrap_run_id),
                ..EngineRunConfig::default()
            },
        )
        .await?;
    drive_run_handle(engine, handle).await
}

async fn drive_run_handle(
    engine: Arc<Engine>,
    handle: surge_orchestrator::engine::handle::RunHandle,
) -> Result<RunOutcome> {
    let surge_orchestrator::engine::handle::RunHandle {
        run_id,
        mut events,
        completion,
    } = handle;

    loop {
        match events.recv().await {
            Ok(EngineRunEvent::Persisted { seq, payload }) => {
                print_bootstrap_event(seq, &payload);
                if let EventPayload::HumanInputRequested {
                    call_id, prompt, ..
                } = payload
                {
                    let response = prompt_for_gate_decision(None, &prompt)?;
                    engine
                        .resolve_human_input(run_id, call_id, response)
                        .await?;
                }
            },
            Ok(EngineRunEvent::Terminal { outcome }) => return Ok(outcome),
            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                eprintln!("note: dropped {n} events while watching run {run_id}");
            },
            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                let outcome = completion
                    .await
                    .map_err(|e| anyhow!("run task join failed: {e}"))?;
                return Ok(outcome);
            },
            Ok(_) => {},
        }
    }
}

async fn poll_console_approvals(
    engine: Arc<Engine>,
    storage: Arc<Storage>,
    run_id: RunId,
) -> Result<()> {
    let mut next_seq = EventSeq(1);
    let mut last_stage = None;

    loop {
        let reader = match storage.open_run_reader(run_id).await {
            Ok(reader) => reader,
            Err(_) => {
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            },
        };
        let current = reader.current_seq().await?;
        if current < next_seq {
            tokio::time::sleep(Duration::from_millis(100)).await;
            continue;
        }

        let events = reader.read_events(next_seq..current.next()).await?;
        for event in events {
            next_seq = event.seq.next();
            match event.payload.payload {
                EventPayload::BootstrapApprovalRequested { stage, .. } => {
                    last_stage = Some(stage);
                },
                EventPayload::HumanInputRequested {
                    call_id, prompt, ..
                } => {
                    let response = prompt_for_gate_decision(last_stage, &prompt)?;
                    engine
                        .resolve_human_input(run_id, call_id, response)
                        .await?;
                },
                EventPayload::RunCompleted { .. }
                | EventPayload::RunFailed { .. }
                | EventPayload::RunAborted { .. } => return Ok(()),
                _ => {},
            }
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn prompt_for_gate_decision(
    stage: Option<BootstrapStage>,
    prompt: &str,
) -> Result<serde_json::Value> {
    println!();
    if let Some(stage) = stage {
        println!("bootstrap approval: {stage:?}");
    } else {
        println!("approval requested");
    }
    if !prompt.is_empty() {
        println!("{prompt}");
    }
    println!("[a] approve  [e] edit  [r] reject");
    print!("choice: ");
    io::stdout().flush()?;

    let mut choice = String::new();
    io::stdin().read_line(&mut choice)?;
    match choice.trim().to_lowercase().as_str() {
        "" | "a" | "approve" => Ok(serde_json::json!({"outcome": "approve"})),
        "e" | "edit" => {
            print!("feedback: ");
            io::stdout().flush()?;
            let mut feedback = String::new();
            io::stdin().read_line(&mut feedback)?;
            Ok(serde_json::json!({
                "outcome": "edit",
                "comment": feedback.trim()
            }))
        },
        "r" | "reject" => {
            print!("reason: ");
            io::stdout().flush()?;
            let mut reason = String::new();
            io::stdin().read_line(&mut reason)?;
            Ok(serde_json::json!({
                "outcome": "reject",
                "comment": reason.trim()
            }))
        },
        other => Err(anyhow!("unknown approval choice: {other}")),
    }
}

async fn build_local_engine(worktree: &Path) -> Result<(Arc<Engine>, Arc<Storage>)> {
    let storage = Storage::open(&surge_home_dir()?)
        .await
        .context("open storage")?;
    let bridge: Arc<dyn surge_acp::bridge::facade::BridgeFacade> = Arc::new(
        surge_acp::bridge::AcpBridge::with_defaults().context("AcpBridge::with_defaults")?,
    );
    let tool_dispatcher: Arc<dyn surge_orchestrator::engine::tools::ToolDispatcher> = Arc::new(
        surge_orchestrator::engine::tools::worktree::WorktreeToolDispatcher::new(
            worktree.to_path_buf(),
        ),
    );
    let notifier = build_default_notifier();
    let profile_registry = Arc::new(
        surge_orchestrator::profile_loader::ProfileRegistry::load()
            .context("load profile registry")?,
    );
    let engine = Arc::new(Engine::new_full(
        bridge,
        storage.clone(),
        tool_dispatcher,
        notifier,
        None,
        Some(profile_registry),
        EngineConfig::default(),
    ));
    Ok((engine, storage))
}

fn create_bootstrap_worktree(run_id: &RunId, worktree_root: Option<PathBuf>) -> Result<PathBuf> {
    let manager = GitManager::discover().context("discover git repository for bootstrap")?;
    let location = bootstrap_worktree_location(worktree_root)?;
    let info = manager
        .create_run_worktree(run_id, None, location)
        .context("create managed bootstrap worktree")?;
    println!("worktree={}", info.path.display());
    Ok(info.path)
}

fn existing_bootstrap_worktree(run_id: &RunId, worktree_root: Option<PathBuf>) -> Result<PathBuf> {
    let manager = GitManager::discover().context("discover git repository for bootstrap")?;
    let location = bootstrap_worktree_location(worktree_root)?;
    manager.find_run_worktree_path(run_id).with_context(|| {
        format!(
            "managed bootstrap worktree does not exist: {}",
            manager.run_worktree_path(run_id, location).display()
        )
    })
}

fn bootstrap_worktree_location(worktree_root: Option<PathBuf>) -> Result<WorktreeLocation> {
    let Some(root) = worktree_root else {
        return Ok(WorktreeLocation::Sibling);
    };
    Ok(WorktreeLocation::Custom(absolute_path(root)?))
}

fn absolute_path(path: PathBuf) -> Result<PathBuf> {
    if path.is_absolute() {
        return Ok(path);
    }
    Ok(std::env::current_dir().context("cwd")?.join(path))
}

fn parse_run_id(s: &str) -> Result<RunId> {
    s.parse()
        .map_err(|e| anyhow!("invalid bootstrap run id '{s}': {e}"))
}

fn surge_home_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| anyhow!("HOME not set"))?;
    let dir = home.join(".surge");
    std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    Ok(dir)
}

fn build_default_notifier() -> Arc<dyn surge_notify::NotifyDeliverer> {
    Arc::new(
        surge_notify::MultiplexingNotifier::new()
            .with_desktop(Arc::new(surge_notify::DesktopDeliverer::new()))
            .with_webhook(Arc::new(surge_notify::WebhookDeliverer::new())),
    )
}

fn print_bootstrap_event(seq: u64, payload: &EventPayload) {
    match payload {
        EventPayload::StageEntered { node, attempt } => {
            eprintln!("[{seq}] stage {node} attempt {attempt}");
        },
        EventPayload::StageCompleted { node, outcome } => {
            eprintln!("[{seq}] stage {node} -> {outcome}");
        },
        EventPayload::ArtifactProduced { name, path, .. } => {
            eprintln!("[{seq}] artifact {name}: {}", path.display());
        },
        EventPayload::PipelineMaterialized { graph, .. } => {
            eprintln!("[{seq}] graph materialized: {}", graph.metadata.name);
        },
        other => eprintln!("[{seq}] {}", other.discriminant_str()),
    }
}
