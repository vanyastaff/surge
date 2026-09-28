//! `surge bootstrap` — adaptive bootstrap flow entrypoint.

use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use clap::{Args, Subcommand};
use surge_core::SurgeConfig;
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
    let (config, project_root) = load_project_config_for_current_repo()?;
    let worktree = create_bootstrap_worktree(&bootstrap_run_id, worktree_root, &config)?;
    let (engine, storage) = build_local_engine(&worktree, &config).await?;
    let project_context =
        surge_orchestrator::project_context::load_project_context_seed(&project_root, &config);

    let approvals = tokio::spawn(poll_console_approvals(
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
            project_context,
            None, // production run: real ~/.surge/memory.db is correct here
        )
        .await
    });

    let materialized = await_bootstrap_driver(driver, approvals, |reason| {
        engine.stop_run(bootstrap_run_id, reason)
    })
    .await?;
    start_followup_run(engine, materialized, worktree, project_root, config).await
}

async fn await_bootstrap_driver<S, C>(
    mut driver: tokio::task::JoinHandle<
        Result<MaterializedRun, surge_orchestrator::bootstrap_driver::BootstrapError>,
    >,
    mut approvals: tokio::task::JoinHandle<Result<()>>,
    stop: S,
) -> Result<MaterializedRun>
where
    S: FnOnce(String) -> C,
    C: std::future::Future<Output = Result<(), surge_orchestrator::engine::EngineError>>,
{
    let materialized = tokio::select! {
        result = &mut driver => {
            approvals.abort();
            let _ = approvals.await;
            result.context("bootstrap driver task panicked")??
        }
        approval_result = &mut approvals => {
            match approval_result.context("approval task panicked").and_then(std::convert::identity) {
                Ok(()) => driver.await.context("bootstrap driver task panicked")??,
                Err(error) => {
                    let driver_result = super::run_lifecycle::stop_and_join(
                        &mut driver,
                        stop(error.to_string()),
                    ).await.with_context(|| error.to_string())?;
                    driver_result.with_context(|| error.to_string())?;
                    return Err(error);
                },
            }
        }
    };
    Ok(materialized)
}

async fn resume_command(run_id: String, worktree_root: Option<PathBuf>) -> Result<()> {
    let bootstrap_run_id = parse_run_id(&run_id)?;
    let (config, project_root) = load_project_config_for_current_repo()?;
    let worktree = existing_bootstrap_worktree(&bootstrap_run_id, worktree_root, &config)?;
    let (engine, storage) = build_local_engine(&worktree, &config).await?;
    let after_seq = storage
        .open_run_reader(bootstrap_run_id)
        .await?
        .current_seq()
        .await?
        .as_u64();
    let events = engine.subscribe_tap();
    let handle = engine
        .resume_run(bootstrap_run_id, worktree.clone())
        .await?;
    let outcome = drive_run_handle(engine.clone(), handle, events, after_seq, |prompt| {
        prompt_for_gate_decision(None, prompt)
    })
    .await?;
    match outcome {
        RunOutcome::Completed { .. } => {},
        RunOutcome::Failed { error } => return Err(anyhow!("bootstrap run failed: {error}")),
        RunOutcome::Aborted { reason } => return Err(anyhow!("bootstrap run aborted: {reason}")),
        RunOutcome::Parked { wake_at } => {
            return Err(anyhow!(
                "bootstrap run parked until {wake_at} (provider rate limit exhausted); the \
                 worktree and event log are intact — run `surge bootstrap resume {bootstrap_run_id}` \
                 again after that time"
            ));
        },
        _ => return Err(anyhow!("bootstrap run reached an unknown terminal outcome")),
    }

    let materialized = materialized_run_from_completed(engine.as_ref(), bootstrap_run_id).await?;
    start_followup_run(engine, materialized, worktree, project_root, config).await
}

async fn start_followup_run(
    engine: Arc<Engine>,
    materialized: MaterializedRun,
    worktree: PathBuf,
    project_root: PathBuf,
    config: SurgeConfig,
) -> Result<()> {
    let followup_run_id = RunId::new();
    println!("followup_run_id={followup_run_id}");
    let events = engine.subscribe_tap();
    let handle = engine
        .start_run(
            followup_run_id,
            materialized.materialized_graph,
            worktree.clone(),
            surge_orchestrator::project_context::with_project_context_seed(
                EngineRunConfig {
                    bootstrap_parent: Some(materialized.bootstrap_run_id),
                    ..EngineRunConfig::default()
                },
                &project_root,
                &config,
            ),
        )
        .await?;
    let outcome = drive_run_handle(engine, handle, events, 0, |prompt| {
        prompt_for_gate_decision(None, prompt)
    })
    .await?;
    super::run_lifecycle::require_completed(followup_run_id, outcome)
}

async fn drive_run_handle<D>(
    engine: Arc<Engine>,
    handle: surge_orchestrator::engine::handle::RunHandle,
    events: tokio::sync::broadcast::Receiver<surge_orchestrator::engine::RunEventTap>,
    after_seq: u64,
    decide: D,
) -> Result<RunOutcome>
where
    D: Fn(&str) -> Result<serde_json::Value>,
{
    let run_id = handle.run_id;
    let decide = &decide;
    super::run_lifecycle::drive_run(
        handle,
        Some(events),
        true,
        |event| {
            let engine = engine.clone();
            async move {
                if let EngineRunEvent::Persisted { seq, payload } = event {
                    print_bootstrap_event(seq, &payload);
                    // A resumed tap replays the entire log. Only newly emitted
                    // requests belong to this execution; replayed answers must
                    // never be submitted to a different pending gate.
                    if seq <= after_seq {
                        return Ok(());
                    }
                    if let EventPayload::HumanInputRequested {
                        node,
                        call_id,
                        prompt,
                        ..
                    } = payload.as_ref()
                    {
                        let response = decide(prompt)?;
                        engine
                            .resolve_requested_input(
                                run_id,
                                node.clone(),
                                call_id.clone(),
                                response,
                            )
                            .await?;
                    }
                }
                Ok(())
            }
        },
        |reason| engine.stop_run(run_id, reason),
    )
    .await
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
                    node,
                    call_id,
                    prompt,
                    ..
                } => {
                    let response = prompt_for_gate_decision(last_stage, &prompt)?;
                    engine
                        .resolve_requested_input(run_id, node, call_id, response)
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

    read_gate_decision(&mut io::stdin().lock(), &mut io::stdout())
}

fn read_gate_decision(
    input: &mut impl io::BufRead,
    output: &mut impl io::Write,
) -> Result<serde_json::Value> {
    let mut choice = String::new();
    if input.read_line(&mut choice)? == 0 {
        return Err(anyhow!(
            "approval input closed (EOF); no approval was submitted. Resume with an interactive bootstrap console"
        ));
    }
    match choice.trim().to_lowercase().as_str() {
        "" | "a" | "approve" => Ok(serde_json::json!({"outcome": "approve"})),
        "e" | "edit" => {
            write!(output, "feedback: ")?;
            output.flush()?;
            let mut feedback = String::new();
            if input.read_line(&mut feedback)? == 0 {
                return Err(anyhow!(
                    "approval feedback input closed (EOF); no decision was submitted"
                ));
            }
            Ok(serde_json::json!({
                "outcome": "edit",
                "comment": feedback.trim()
            }))
        },
        "r" | "reject" => {
            write!(output, "reason: ")?;
            output.flush()?;
            let mut reason = String::new();
            if input.read_line(&mut reason)? == 0 {
                return Err(anyhow!(
                    "approval reason input closed (EOF); no decision was submitted"
                ));
            }
            Ok(serde_json::json!({
                "outcome": "reject",
                "comment": reason.trim()
            }))
        },
        other => Err(anyhow!("unknown approval choice: {other}")),
    }
}

async fn build_local_engine(
    worktree: &Path,
    config: &SurgeConfig,
) -> Result<(Arc<Engine>, Arc<Storage>)> {
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
        // Task 12 M3, acceptance criterion B: bootstrap runs dispatch
        // agent nodes exactly like a follow-up run, so they get the same
        // `[capacity]`-derived policy rather than the hardcoded default.
        EngineConfig {
            capacity: (&config.capacity).into(),
            // Bootstrap dispatches agent nodes exactly like a follow-up run,
            // so it resolves providers through the same unified catalog
            // (user `[agents.*]` over builtins).
            agent_registry: Some(std::sync::Arc::new(surge_acp::Registry::for_run(config))),
            ..EngineConfig::default()
        },
    ));
    Ok((engine, storage))
}

fn create_bootstrap_worktree(
    run_id: &RunId,
    worktree_root: Option<PathBuf>,
    config: &SurgeConfig,
) -> Result<PathBuf> {
    let manager = GitManager::discover().context("discover git repository for bootstrap")?;
    let location = bootstrap_worktree_location(worktree_root, config, manager.repo_path())?;
    let info = manager
        .create_run_worktree(run_id, None, location)
        .context("create managed bootstrap worktree")?;
    println!("worktree={}", info.path.display());
    Ok(info.path)
}

fn existing_bootstrap_worktree(
    run_id: &RunId,
    worktree_root: Option<PathBuf>,
    config: &SurgeConfig,
) -> Result<PathBuf> {
    let manager = GitManager::discover().context("discover git repository for bootstrap")?;
    let location = bootstrap_worktree_location(worktree_root, config, manager.repo_path())?;
    manager.find_run_worktree_path(run_id).with_context(|| {
        format!(
            "managed bootstrap worktree does not exist: {}",
            manager.run_worktree_path(run_id, location).display()
        )
    })
}

fn bootstrap_worktree_location(
    worktree_root: Option<PathBuf>,
    config: &SurgeConfig,
    project_root: &Path,
) -> Result<WorktreeLocation> {
    if let Some(root) = worktree_root {
        return Ok(WorktreeLocation::Custom(absolute_path(root)?));
    }
    match config.init.worktree_location {
        surge_core::config::WorktreeLocationConfig::Sibling => Ok(WorktreeLocation::Sibling),
        surge_core::config::WorktreeLocationConfig::Central => Ok(WorktreeLocation::Central),
        surge_core::config::WorktreeLocationConfig::Custom => Ok(WorktreeLocation::Custom(
            absolute_path_from(project_root, config.init.worktree_root.clone()),
        )),
    }
}

fn absolute_path(path: PathBuf) -> Result<PathBuf> {
    if path.is_absolute() {
        return Ok(path);
    }
    Ok(std::env::current_dir().context("cwd")?.join(path))
}

fn absolute_path_from(base: &Path, path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        base.join(path)
    }
}

fn load_project_config_for_current_repo() -> Result<(SurgeConfig, PathBuf)> {
    let project_root = GitManager::discover()
        .map(|manager| manager.repo_path().to_path_buf())
        .unwrap_or_else(|_| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let config_path = project_root.join("surge.toml");
    let config = if config_path.exists() {
        SurgeConfig::load(&config_path)
            .with_context(|| format!("load {}", config_path.display()))?
    } else {
        SurgeConfig::load_or_default().context("load surge config")?
    };
    Ok((config, project_root))
}

fn parse_run_id(s: &str) -> Result<RunId> {
    s.parse()
        .map_err(|e| anyhow!("invalid bootstrap run id '{s}': {e}"))
}

fn surge_home_dir() -> Result<PathBuf> {
    // Honour SURGE_HOME like the rest of the orchestrator so tests can
    // isolate per-tempdir; see `feature::surge_home_dir` for the same note.
    let dir = if let Ok(custom) = std::env::var("SURGE_HOME") {
        if !custom.is_empty() {
            PathBuf::from(custom)
        } else {
            dirs::home_dir()
                .ok_or_else(|| anyhow!("SURGE_HOME unset and home directory unknown"))?
                .join(".surge")
        }
    } else {
        dirs::home_dir()
            .ok_or_else(|| anyhow!("SURGE_HOME unset and home directory unknown"))?
            .join(".surge")
    };
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

#[cfg(test)]
mod tests {
    use super::*;
    use surge_core::config::WorktreeLocationConfig;

    #[test]
    fn gate_eof_never_approves() {
        for text in ["", "edit\n", "reject\n"] {
            let error = read_gate_decision(&mut text.as_bytes(), &mut Vec::new()).unwrap_err();
            assert!(error.to_string().contains("EOF"));
        }
    }

    #[tokio::test]
    async fn approval_failure_settles_driver_and_preserves_its_failure() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let settled = Arc::new(AtomicBool::new(false));
        let settled_by_driver = settled.clone();
        let (cancel, cancelled) = tokio::sync::oneshot::channel();
        let driver = tokio::spawn(async move {
            cancelled.await.unwrap();
            settled_by_driver.store(true, Ordering::SeqCst);
            Err(
                surge_orchestrator::bootstrap_driver::BootstrapError::RunFailed(
                    "persist RunAborted: disk failure".into(),
                ),
            )
        });
        let approvals = tokio::spawn(async { Err(anyhow!("approval input closed")) });
        let error = tokio::time::timeout(
            Duration::from_secs(2),
            await_bootstrap_driver(driver, approvals, |_| async move {
                cancel.send(()).unwrap();
                Ok(())
            }),
        )
        .await
        .unwrap()
        .unwrap_err();
        assert!(settled.load(Ordering::SeqCst));
        let message = format!("{error:#}");
        assert!(message.contains("approval input closed"), "{message}");
        assert!(
            message.contains("persist RunAborted: disk failure"),
            "{message}"
        );
    }

    #[tokio::test]
    async fn finished_approval_loop_waits_for_successful_driver() {
        let id = RunId::new();
        let driver = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            Ok(MaterializedRun {
                bootstrap_run_id: id,
                materialized_graph: toml::from_str(include_str!(
                    "../../../../examples/flow_terminal_only.toml"
                ))
                .unwrap(),
                artifacts: vec![],
            })
        });
        let approvals = tokio::spawn(async { Ok(()) });
        let run = tokio::time::timeout(
            Duration::from_secs(2),
            await_bootstrap_driver(driver, approvals, |_| async {
                panic!("normal completion must not cancel")
            }),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(run.bootstrap_run_id, id);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn resumed_console_answers_only_the_fresh_pending_gate() {
        let temp = tempfile::tempdir().unwrap();
        let storage = Storage::open(temp.path()).await.unwrap();
        let bridge = Arc::new(surge_acp::bridge::AcpBridge::with_defaults().unwrap());
        let build_engine = || {
            Arc::new(Engine::new(
                bridge.clone(),
                storage.clone(),
                Arc::new(
                    surge_orchestrator::engine::tools::worktree::WorktreeToolDispatcher::new(
                        temp.path().to_path_buf(),
                    ),
                ),
                EngineConfig::default(),
            ))
        };
        let engine = build_engine();
        let mut graph: surge_core::graph::Graph =
            toml::from_str(include_str!("../../../../examples/flow_terminal_only.toml")).unwrap();
        graph.start = "first".try_into().unwrap();
        for (name, target) in [("first", "second"), ("second", "end")] {
            let node: surge_core::node::Node = serde_json::from_value(serde_json::json!({
                "id": name, "position": {"x": 0.0, "y": 0.0},
                "declared_outcomes": [{"id": "approve", "description": "approved", "edge_kind_hint": "forward", "is_terminal": false}],
                "config": {"node_kind": "human_gate", "delivery_channels": [],
                    "summary": {"title": name, "body": name},
                    "options": [{"outcome": "approve", "label": "Approve"}]},
            })).unwrap();
            graph.nodes.insert(node.id.clone(), node);
            graph.edges.push(
                serde_json::from_value(serde_json::json!({
                    "id": name, "from": {"node": name, "outcome": "approve"},
                    "to": target, "kind": "forward",
                }))
                .unwrap(),
            );
        }
        let id = RunId::new();
        let mut events = engine.subscribe_tap();
        let handle = engine
            .start_run(
                id,
                graph,
                temp.path().to_path_buf(),
                EngineRunConfig::default(),
            )
            .await
            .unwrap();
        let (node, call_id) = wait_for_gate(&mut events, "first").await;
        engine
            .resolve_requested_input(id, node, call_id, serde_json::json!({"outcome": "approve"}))
            .await
            .unwrap();
        wait_for_gate(&mut events, "second").await;
        // Simulate interruption without a terminal event; resume replays history.
        handle.completion.abort();
        assert!(handle.completion.await.unwrap_err().is_cancelled());
        drop(engine);
        let resumed_engine = build_engine();
        let after_seq = storage
            .open_run_reader(id)
            .await
            .unwrap()
            .current_seq()
            .await
            .unwrap()
            .as_u64();
        let events = resumed_engine.subscribe_tap();
        let handle = resumed_engine
            .resume_run(id, temp.path().to_path_buf())
            .await
            .unwrap();
        let prompts = std::sync::Mutex::new(Vec::new());
        let outcome = tokio::time::timeout(
            Duration::from_secs(5),
            drive_run_handle(resumed_engine, handle, events, after_seq, |prompt| {
                prompts.lock().unwrap().push(prompt.to_string());
                Ok(serde_json::json!({"outcome": "approve"}))
            }),
        )
        .await
        .unwrap();
        let prompts = prompts.into_inner().unwrap();
        assert_eq!(prompts.len(), 1, "{prompts:?}; outcome: {outcome:?}");
        assert!(prompts[0].contains("second"), "{prompts:?}");
        assert!(matches!(outcome.unwrap(), RunOutcome::Completed { .. }));
    }

    async fn wait_for_gate(
        events: &mut tokio::sync::broadcast::Receiver<surge_orchestrator::engine::RunEventTap>,
        expected: &str,
    ) -> (surge_core::keys::NodeKey, Option<String>) {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let event = events.recv().await.unwrap();
                if let EventPayload::HumanInputRequested { node, call_id, .. } =
                    event.event.payload.payload
                    && node.as_str() == expected
                {
                    break (node, call_id);
                }
            }
        })
        .await
        .unwrap()
    }

    #[test]
    fn gate_explicit_decisions_and_blank_enter() {
        for (text, outcome, comment) in [
            ("approve\n", "approve", None),
            ("\n", "approve", None),
            ("edit\nfix the plan\n", "edit", Some("fix the plan")),
            ("reject\nwrong task\n", "reject", Some("wrong task")),
        ] {
            let decision = read_gate_decision(&mut text.as_bytes(), &mut Vec::new()).unwrap();
            assert_eq!(decision["outcome"], outcome);
            assert_eq!(
                decision.get("comment").and_then(serde_json::Value::as_str),
                comment
            );
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn followup_failure_reaches_the_command_result() {
        use surge_core::run_event::VersionedEventPayload;
        use surge_persistence::artifacts::ArtifactStore;

        let temp = tempfile::tempdir().unwrap();
        let storage = Storage::open(temp.path()).await.unwrap();
        let engine = Arc::new(Engine::new(
            Arc::new(surge_acp::bridge::AcpBridge::with_defaults().unwrap()),
            storage.clone(),
            Arc::new(
                surge_orchestrator::engine::tools::worktree::WorktreeToolDispatcher::new(
                    temp.path().to_path_buf(),
                ),
            ),
            EngineConfig::default(),
        ));
        let graph: surge_core::graph::Graph =
            toml::from_str(include_str!("../../../../examples/flow_terminal_only.toml")).unwrap();
        let parent = RunId::new();
        engine
            .start_run(
                parent,
                graph.clone(),
                temp.path().to_path_buf(),
                EngineRunConfig::default(),
            )
            .await
            .unwrap()
            .await_completion()
            .await
            .unwrap();
        let writer = storage.open_run_writer(parent).await.unwrap();
        let artifacts = ArtifactStore::new(temp.path().join("runs"));
        for name in ["description", "roadmap", "flow"] {
            let reference = artifacts.put(parent, name, b"fixture").await.unwrap();
            writer
                .append_event(VersionedEventPayload::new(EventPayload::ArtifactProduced {
                    node: "end".try_into().unwrap(),
                    artifact: reference.hash,
                    path: reference.path,
                    name: name.into(),
                    source_path: None,
                }))
                .await
                .unwrap();
        }
        writer.close().await.unwrap();
        let mut failure = graph;
        failure.nodes.get_mut(&failure.start).unwrap().config =
            surge_core::node::NodeConfig::Terminal(surge_core::terminal_config::TerminalConfig {
                kind: surge_core::terminal_config::TerminalKind::Failure { exit_code: 1 },
                message: None,
            });
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            start_followup_run(
                engine,
                MaterializedRun {
                    bootstrap_run_id: parent,
                    materialized_graph: failure,
                    artifacts: vec![],
                },
                temp.path().to_path_buf(),
                temp.path().to_path_buf(),
                SurgeConfig::default(),
            ),
        )
        .await
        .unwrap();
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("failed: terminal failure node")
        );
    }

    #[test]
    fn bootstrap_worktree_location_uses_config_default() {
        let project = tempfile::tempdir().unwrap();
        let mut config = SurgeConfig::default();
        config.init.worktree_location = WorktreeLocationConfig::Central;

        let location = bootstrap_worktree_location(None, &config, project.path()).unwrap();

        assert!(matches!(location, WorktreeLocation::Central));
    }

    #[test]
    fn bootstrap_worktree_location_resolves_custom_root_from_project_root() {
        let project = tempfile::tempdir().unwrap();
        let mut config = SurgeConfig::default();
        config.init.worktree_location = WorktreeLocationConfig::Custom;
        config.init.worktree_root = PathBuf::from(".runs");

        let location = bootstrap_worktree_location(None, &config, project.path()).unwrap();

        match location {
            WorktreeLocation::Custom(path) => assert_eq!(path, project.path().join(".runs")),
            other => panic!("expected custom worktree location, got {other:?}"),
        }
    }

    #[test]
    fn bootstrap_worktree_location_prefers_cli_override() {
        let project = tempfile::tempdir().unwrap();
        let override_root = project.path().join("override");
        let mut config = SurgeConfig::default();
        config.init.worktree_location = WorktreeLocationConfig::Central;

        let location =
            bootstrap_worktree_location(Some(override_root.clone()), &config, project.path())
                .unwrap();

        match location {
            WorktreeLocation::Custom(path) => assert_eq!(path, override_root),
            other => panic!("expected custom worktree location, got {other:?}"),
        }
    }
}
